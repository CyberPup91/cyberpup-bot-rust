use crate::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateSelectMenu,
    CreateSelectMenuKind, CreateSelectMenuOption, InputTextStyle,
};
use serenity::utils::CreateQuickModal;

fn trusted_user(user_id: u64) -> bool {
    std::env::var("TRUSTED_USER_IDS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|id| id.trim().parse::<u64>().ok())
        .any(|id| id == user_id)
}

fn parse_ids(value: &str) -> Vec<u64> {
    value
        .split(',')
        .filter_map(|part| part.trim().parse::<u64>().ok())
        .collect()
}

fn ids_string(values: &[u64]) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn authorized(user_id: u64, is_admin: bool) -> bool {
    trusted_user(user_id) || is_admin
}

fn dashboard_embed(commands: &[super::database::CustomCommand]) -> CreateEmbed {
    let description = if commands.is_empty() {
        "No custom commands have been created in this server yet.\n\nPress **Add Command** to create your first one.".to_string()
    } else {
        commands
            .iter()
            .take(20)
            .map(|command| {
                let status = if command.is_active { "🟢" } else { "⚪" };
                format!("{status} `!{}`", command.name)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    CreateEmbed::new()
        .title("🐾 Custom Commands")
        .description(description)
        .color(0x9B59B6)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "{} command(s) in this server",
            commands.len()
        )))
}

fn dashboard_components(commands: &[super::database::CustomCommand]) -> Vec<CreateActionRow> {
    let mut rows = Vec::new();

    if !commands.is_empty() {
        let options = commands
            .iter()
            .take(25)
            .map(|command| {
                CreateSelectMenuOption::new(format!("!{}", command.name), command.id.to_string())
                    .description(format!(
                        "{} · {}s cooldown",
                        if command.is_active {
                            "Active"
                        } else {
                            "Inactive"
                        },
                        command.cooldown
                    ))
            })
            .collect();

        rows.push(CreateActionRow::SelectMenu(
            CreateSelectMenu::new("cc:select", CreateSelectMenuKind::String { options })
                .placeholder("Choose a command to manage"),
        ));
    }

    rows.push(CreateActionRow::Buttons(vec![
        CreateButton::new("cc:add")
            .label("Add Command")
            .style(ButtonStyle::Success),
        CreateButton::new("cc:refresh")
            .label("Refresh")
            .style(ButtonStyle::Secondary),
    ]));

    rows
}

fn command_embed(command: &super::database::CustomCommand) -> CreateEmbed {
    let roles = if command.allowed_roles.trim().is_empty() {
        "Everyone".to_string()
    } else {
        command
            .allowed_roles
            .split(',')
            .map(|id| format!("<@&{}>", id.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let users = if command.allowed_users.trim().is_empty() {
        "Everyone".to_string()
    } else {
        command
            .allowed_users
            .split(',')
            .map(|id| format!("<@{}>", id.trim()))
            .collect::<Vec<_>>()
            .join(", ")
    };

    CreateEmbed::new()
        .title(format!("⚙️ !{}", command.name))
        .description(format!("```text\n{}\n```", command.response))
        .color(0x9B59B6)
        .field(
            "Global Cooldown",
            format!("{} seconds", command.cooldown),
            true,
        )
        .field(
            "User Cooldown",
            format!("{} seconds", command.user_cooldown),
            true,
        )
        .field(
            "Response Mode",
            if command.should_reply {
                "Reply to message"
            } else {
                "Send normally"
            },
            true,
        )
        .field("Allowed Roles", roles, false)
        .field("Allowed Users", users, false)
        .field("Counter", command.counter.to_string(), true)
        .field(
            "Status",
            if command.is_active {
                "Active"
            } else {
                "Inactive"
            },
            true,
        )
}

fn selected_components(command_id: i64) -> Vec<CreateActionRow> {
    vec![
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("cc:edit:{command_id}"))
                .label("Edit")
                .style(ButtonStyle::Primary),
            CreateButton::new(format!("cc:reply:{command_id}"))
                .label("Toggle Reply")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("cc:delete:{command_id}"))
                .label("Delete")
                .style(ButtonStyle::Danger),
        ]),
        CreateActionRow::Buttons(vec![CreateButton::new("cc:back")
            .label("Back to Commands")
            .style(ButtonStyle::Secondary)]),
    ]
}

async fn update_dashboard(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
) -> Result<(), Error> {
    let commands = super::database::list_commands(&data.custom_commands_db, guild_id).await?;

    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(dashboard_embed(&commands))
                    .components(dashboard_components(&commands)),
            ),
        )
        .await?;

    Ok(())
}

async fn show_command(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
    command_id: i64,
) -> Result<(), Error> {
    let Some(command) =
        super::database::get_by_id(&data.custom_commands_db, guild_id, command_id).await?
    else {
        return respond(ctx, interaction, "That command no longer exists.").await;
    };

    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .embed(command_embed(&command))
                    .components(selected_components(command_id)),
            ),
        )
        .await?;

    Ok(())
}

async fn open_editor(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
    existing: Option<super::database::CustomCommand>,
) -> Result<(), Error> {
    let editing = existing.is_some();

    let (name, response, cooldown, user_cooldown, allowed_ids) =
        if let Some(command) = existing.as_ref() {
            let mut ids = parse_ids(&command.allowed_roles);
            ids.extend(parse_ids(&command.allowed_users));

            (
                command.name.clone(),
                command.response.clone(),
                command.cooldown.to_string(),
                command.user_cooldown.to_string(),
                ids_string(&ids),
            )
        } else {
            (
                String::new(),
                String::new(),
                "5".to_string(),
                "0".to_string(),
                String::new(),
            )
        };

    let modal = CreateQuickModal::new(if editing {
        "Edit Custom Command"
    } else {
        "Add Custom Command"
    })
    .field(CreateInputText::new(InputTextStyle::Short, "Command Name", "example").value(name))
    .field(
        CreateInputText::new(
            InputTextStyle::Paragraph,
            "Response",
            "Enter the command response",
        )
        .value(response),
    )
    .field(
        CreateInputText::new(InputTextStyle::Short, "Global Cooldown (seconds)", "5")
            .value(cooldown),
    )
    .field(
        CreateInputText::new(InputTextStyle::Short, "User Cooldown (seconds)", "0")
            .value(user_cooldown),
    )
    .field(
        CreateInputText::new(
            InputTextStyle::Short,
            "Allowed Role/User IDs",
            "Comma-separated IDs; leave empty for everyone",
        )
        .required(false)
        .value(allowed_ids),
    );

    if let Some(modal_response) = interaction.quick_modal(ctx, modal).await? {
        let values = &modal_response.inputs;

        if values.len() < 5 {
            respond_modal(
                ctx,
                &modal_response.interaction,
                "The form was incomplete. Please try again.",
            )
            .await?;
            return Ok(());
        }

        let name = values[0].trim().trim_start_matches('!').to_lowercase();

        let response = values[1].trim();
        let cooldown = values[2].trim().parse::<i64>();
        let user_cooldown = values[3].trim().parse::<i64>();
        let access_ids = parse_ids(&values[4]);

        if name.is_empty() || name.len() > 32 || name.contains(char::is_whitespace) {
            respond_modal(
                ctx,
                &modal_response.interaction,
                "Command names must be 1–32 characters with no spaces.",
            )
            .await?;
            return Ok(());
        }

        if response.is_empty() || response.len() > 1000 {
            respond_modal(
                ctx,
                &modal_response.interaction,
                "Responses must contain between 1 and 1000 characters.",
            )
            .await?;
            return Ok(());
        }

        let (Ok(cooldown), Ok(user_cooldown)) = (cooldown, user_cooldown) else {
            respond_modal(
                ctx,
                &modal_response.interaction,
                "Cooldowns must be whole numbers.",
            )
            .await?;
            return Ok(());
        };

        if !(0..=86400).contains(&cooldown) || !(0..=86400).contains(&user_cooldown) {
            respond_modal(
                ctx,
                &modal_response.interaction,
                "Cooldowns must be between 0 and 86400 seconds.",
            )
            .await?;
            return Ok(());
        }

        // Copy role IDs out of the cache before any database awaits.
        let guild_role_ids: std::collections::HashSet<u64> = {
            ctx.cache
                .guild(serenity::GuildId::new(guild_id))
                .map(|guild| guild.roles.keys().map(|role_id| role_id.get()).collect())
                .unwrap_or_default()
        };

        let mut roles = Vec::new();
        let mut users = Vec::new();

        for id in access_ids {
            if guild_role_ids.contains(&id) {
                roles.push(id);
            } else {
                users.push(id);
            }
        }

        let result = if let Some(command) = existing {
            sqlx::query(
                "UPDATE custom_commands
                 SET name = ?, response = ?, cooldown = ?, user_cooldown = ?,
                     allowed_roles = ?, allowed_users = ?
                 WHERE guild_id = ? AND id = ?",
            )
            .bind(&name)
            .bind(response)
            .bind(cooldown)
            .bind(user_cooldown)
            .bind(ids_string(&roles))
            .bind(ids_string(&users))
            .bind(guild_id as i64)
            .bind(command.id)
            .execute(&data.custom_commands_db)
            .await
        } else {
            sqlx::query(
                "INSERT INTO custom_commands
                 (guild_id, name, response, cooldown, user_cooldown, allowed_roles, allowed_users)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(guild_id as i64)
            .bind(&name)
            .bind(response)
            .bind(cooldown)
            .bind(user_cooldown)
            .bind(ids_string(&roles))
            .bind(ids_string(&users))
            .execute(&data.custom_commands_db)
            .await
        };

        match result {
            Ok(_) => {
                let commands =
                    super::database::list_commands(&data.custom_commands_db, guild_id).await?;

                modal_response
                    .interaction
                    .create_response(
                        ctx,
                        CreateInteractionResponse::UpdateMessage(
                            CreateInteractionResponseMessage::new()
                                .embed(dashboard_embed(&commands))
                                .components(dashboard_components(&commands)),
                        ),
                    )
                    .await?;
            }
            Err(error) => {
                tracing::warn!("Could not save custom command: {error}");
                respond_modal(
                    ctx,
                    &modal_response.interaction,
                    "Could not save that command. It may already exist in this server.",
                )
                .await?;
            }
        }
    }

    Ok(())
}

async fn respond(
    ctx: &serenity::Context,
    interaction: &serenity::all::ComponentInteraction,
    content: &str,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

async fn respond_modal(
    ctx: &serenity::Context,
    interaction: &serenity::all::ModalInteraction,
    content: &str,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

/// Open the private custom-command management dashboard.
#[poise::command(slash_command, guild_only)]
pub async fn cc_admin(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command requires a server.")?
        .get();

    let member = ctx.author_member().await;

    let is_admin = {
        match (ctx.guild(), member) {
            (Some(guild), Some(member)) => guild.member_permissions(&member).administrator(),
            _ => false,
        }
    };

    if !authorized(ctx.author().id.get(), is_admin) {
        ctx.send(
            poise::CreateReply::default()
                .content("You aren't authorized to manage custom commands.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let commands = super::database::list_commands(&ctx.data().custom_commands_db, guild_id).await?;

    ctx.send(
        poise::CreateReply::default()
            .embed(dashboard_embed(&commands))
            .components(dashboard_components(&commands))
            .ephemeral(true),
    )
    .await?;

    Ok(())
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<bool, Error> {
    if !interaction.data.custom_id.starts_with("cc:") {
        return Ok(false);
    }

    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        respond(ctx, interaction, "This dashboard only works in a server.").await?;
        return Ok(true);
    };

    let is_admin = interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(|permissions| permissions.administrator());

    if !authorized(interaction.user.id.get(), is_admin) {
        respond(
            ctx,
            interaction,
            "You aren't authorized to manage custom commands.",
        )
        .await?;
        return Ok(true);
    }

    let custom_id = interaction.data.custom_id.as_str();

    if custom_id == "cc:refresh" || custom_id == "cc:back" {
        update_dashboard(ctx, data, interaction, guild_id).await?;
    } else if custom_id == "cc:add" {
        open_editor(ctx, data, interaction, guild_id, None).await?;
    } else if custom_id == "cc:select" {
        let serenity::ComponentInteractionDataKind::StringSelect { values } =
            &interaction.data.kind
        else {
            return Ok(true);
        };

        let Some(value) = values.first() else {
            return Ok(true);
        };

        if let Ok(command_id) = value.parse::<i64>() {
            show_command(ctx, data, interaction, guild_id, command_id).await?;
        } else {
            respond(ctx, interaction, "Invalid command selection.").await?;
        }
    } else if let Some(id) = custom_id.strip_prefix("cc:edit:") {
        if let Ok(command_id) = id.parse::<i64>() {
            let command =
                super::database::get_by_id(&data.custom_commands_db, guild_id, command_id).await?;

            if let Some(command) = command {
                open_editor(ctx, data, interaction, guild_id, Some(command)).await?;
            } else {
                respond(ctx, interaction, "That command no longer exists.").await?;
            }
        }
    } else if let Some(id) = custom_id.strip_prefix("cc:reply:") {
        if let Ok(command_id) = id.parse::<i64>() {
            let command =
                super::database::get_by_id(&data.custom_commands_db, guild_id, command_id).await?;

            if let Some(command) = command {
                sqlx::query(
                    "UPDATE custom_commands SET should_reply = ? WHERE guild_id = ? AND id = ?",
                )
                .bind(i64::from(!command.should_reply))
                .bind(guild_id as i64)
                .bind(command_id)
                .execute(&data.custom_commands_db)
                .await?;

                show_command(ctx, data, interaction, guild_id, command_id).await?;
            } else {
                respond(ctx, interaction, "That command no longer exists.").await?;
            }
        }
    } else if let Some(id) = custom_id.strip_prefix("cc:delete:") {
        if let Ok(command_id) = id.parse::<i64>() {
            let Some(command) =
                super::database::get_by_id(&data.custom_commands_db, guild_id, command_id).await?
            else {
                respond(ctx, interaction, "That command no longer exists.").await?;
                return Ok(true);
            };

            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(
                                CreateEmbed::new()
                                    .title("Delete Custom Command?")
                                    .description(format!(
                                        "Are you sure you want to delete `!{}`? This cannot be undone.",
                                        command.name
                                    ))
                                    .color(0xE74C3C),
                            )
                            .components(vec![CreateActionRow::Buttons(vec![
                                CreateButton::new(format!("cc:deleteconfirm:{command_id}"))
                                    .label("Yes, Delete")
                                    .style(ButtonStyle::Danger),
                                CreateButton::new(format!("cc:edit:{command_id}"))
                                    .label("Cancel")
                                    .style(ButtonStyle::Secondary),
                            ])]),
                    ),
                )
                .await?;
        }
    } else if let Some(id) = custom_id.strip_prefix("cc:deleteconfirm:") {
        if let Ok(command_id) = id.parse::<i64>() {
            super::database::delete_command(&data.custom_commands_db, guild_id, command_id).await?;

            update_dashboard(ctx, data, interaction, guild_id).await?;
        }
    } else {
        respond(ctx, interaction, "Unknown custom-command action.").await?;
    }

    Ok(true)
}
