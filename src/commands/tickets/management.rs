use crate::{Context, Error};
use futures::StreamExt;
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditChannel,
    InputTextStyle, PermissionOverwrite, PermissionOverwriteType, Permissions, ReactionType,
    RoleId, UserId,
};
use serenity::collector::ComponentInteractionCollector;
use serenity::utils::CreateQuickModal;
use serenity::Mentionable;
use std::time::Duration;

const PANEL_TIMEOUT: Duration = Duration::from_secs(300);

#[poise::command(
    slash_command,
    subcommands("manage", "open", "add", "remove", "rename")
)]
pub async fn ticket(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(slash_command, required_permissions = "MANAGE_GUILD")]
pub async fn manage(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id().map(|id| id.get()) else {
        ctx.say("This command can only be used inside a server.")
            .await?;
        return Ok(());
    };
    super::database::ensure_config(&ctx.data().tickets_db, guild_id).await?;
    let embed = dashboard_embed(ctx, guild_id).await?;
    let components = dashboard_components();
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(embed)
                .components(components)
                .ephemeral(true),
        )
        .await?;
    let message = reply.message().await?;

    let collector = ComponentInteractionCollector::new(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .message_id(message.id)
        .filter(|interaction| interaction.data.custom_id.starts_with("tickets:manage:"))
        .timeout(PANEL_TIMEOUT);

    let mut stream = Box::pin(collector.stream());
    while let Some(interaction) = stream.next().await {
        let id = interaction.data.custom_id.clone();
        let action = id.strip_prefix("tickets:manage:").unwrap_or_default();
        let result = match action {
            "core" => configure_core(ctx, &interaction, guild_id).await,
            "behavior" => configure_behavior(ctx, &interaction, guild_id).await,
            "embed" => configure_embed(ctx, &interaction, guild_id).await,
            "add_option" => add_option(ctx, &interaction, guild_id).await,
            "remove_option" => delete_option(ctx, &interaction, guild_id).await,
            "deploy" => deploy_panel(ctx, &interaction, guild_id).await,
            _ => Ok(()),
        };
        if let Err(error) = result {
            tracing::error!("Ticket dashboard action failed: {error}");
            if interaction
                .get_response(ctx.serenity_context())
                .await
                .is_err()
            {
                let _ = respond_component(
                    ctx,
                    &interaction,
                    "❌ The action failed. Check the bot logs.",
                )
                .await;
            }
        } else {
            match dashboard_embed(ctx, guild_id).await {
                Ok(embed) => {
                    if let Err(error) = ctx
                        .channel_id()
                        .edit_message(
                            ctx.serenity_context(),
                            message.id,
                            serenity::EditMessage::new()
                                .embed(embed)
                                .components(dashboard_components()),
                        )
                        .await
                    {
                        tracing::warn!("Could not refresh ticket dashboard: {error}");
                    }
                }
                Err(error) => tracing::warn!("Could not rebuild ticket dashboard: {error}"),
            }
        }
    }
    Ok(())
}

async fn dashboard_embed(ctx: Context<'_>, guild_id: u64) -> Result<CreateEmbed, Error> {
    let config = super::database::get_config(&ctx.data().tickets_db, guild_id).await?;
    let options = super::database::list_options(&ctx.data().tickets_db, guild_id).await?;
    let guild = ctx.partial_guild().await;
    let guild_name = guild
        .as_ref()
        .map(|guild| guild.name.as_str())
        .unwrap_or("Server");
    let mut embed = CreateEmbed::new()
        .title(format!("🎫 Ticket Control Dashboard: {guild_name}"))
        .description(
            "Configure ticket routing, behavior, panel appearance, and support categories.",
        )
        .color(0x5865F2)
        .timestamp(serenity::Timestamp::now());
    if let Some(config) = config {
        embed = embed
            .field(
                "📂 Core Routing",
                format!(
                    "**Category:** {}\n**Staff:** {}\n**Logs:** {}\n**Blocked role:** {}",
                    mention_channel(config.category_id),
                    mention_role(config.support_role_id),
                    mention_channel(config.log_channel_id),
                    mention_role(config.block_role_id)
                ),
                true,
            )
            .field(
                "🛡️ Behavior",
                format!(
                    "**Max open tickets:** {}\n**Welcome:** {}",
                    config.max_user_tickets,
                    truncate(&config.ticket_welcome_msg, 80)
                ),
                true,
            )
            .field(
                "🎨 Panel",
                format!(
                    "**Title:** {}\n**Footer:** {}\n**Description:** {}",
                    truncate(&config.panel_title, 60),
                    truncate(&config.panel_footer, 60),
                    truncate(&config.panel_description, 120)
                ),
                false,
            );
    } else {
        embed = embed.field(
            "⚠️ Setup required",
            "Use **Core Routing** to configure a ticket category and support role.",
            false,
        );
    }
    let options_text = if options.is_empty() {
        "Default support category only (single-button panel).".to_string()
    } else {
        options
            .iter()
            .map(|option| {
                format!(
                    "• {}**{}**{} — ID `{}`{}",
                    option
                        .emoji
                        .as_deref()
                        .map(|emoji| format!("{emoji} "))
                        .unwrap_or_default(),
                    option.label,
                    option
                        .support_role_id
                        .map(|id| format!(" (staff override <@&{id}>)"))
                        .unwrap_or_default(),
                    option.id,
                    option
                        .description
                        .as_deref()
                        .map(|description| format!(" — {}", truncate(description, 80)))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    embed = embed.field(
        "📑 Support Categories",
        truncate(&options_text, 1000),
        false,
    );
    Ok(embed)
}

fn dashboard_components() -> Vec<CreateActionRow> {
    let rows = vec![
        CreateActionRow::Buttons(vec![
            button("core", "Core Routing", ButtonStyle::Primary, "⚙️"),
            button("behavior", "Behavior & Limits", ButtonStyle::Primary, "🛡️"),
            button("embed", "Customize Embed", ButtonStyle::Secondary, "🎨"),
        ]),
        CreateActionRow::Buttons(vec![
            button("add_option", "Add Category", ButtonStyle::Success, "➕"),
            button(
                "remove_option",
                "Delete Category",
                ButtonStyle::Danger,
                "🗑️",
            ),
            button("deploy", "Deploy Support Panel", ButtonStyle::Primary, "🚀"),
        ]),
    ];
    rows
}

fn button(action: &str, label: &str, style: ButtonStyle, emoji: &str) -> CreateButton {
    CreateButton::new(format!("tickets:manage:{action}"))
        .label(label)
        .style(style)
        .emoji(ReactionType::Unicode(emoji.into()))
}

async fn configure_core(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
) -> Result<(), Error> {
    let current = super::database::get_config(&ctx.data().tickets_db, guild_id).await?;
    let modal = CreateQuickModal::new("Configure Ticket Routing")
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Category ID",
                "Category ID for new ticket channels",
            )
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.category_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            )
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Support Role ID",
                "Default staff role allowed to view tickets",
            )
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.support_role_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            )
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Log Channel ID",
                "Channel for ticket transcripts",
            )
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.log_channel_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            )
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Restricted Role ID (optional)",
                "Members with this role cannot open tickets",
            )
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.block_role_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            )
            .required(false),
        );
    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        if response.inputs.len() < 4 {
            return Ok(());
        }
        let parsed: Result<Vec<Option<u64>>, _> = response
            .inputs
            .iter()
            .map(|value| {
                if value.trim().is_empty() {
                    Ok(None)
                } else {
                    value.trim().parse::<u64>().map(Some)
                }
            })
            .collect();
        let Ok(values) = parsed else {
            respond_modal(
                ctx,
                &response.interaction,
                "❌ IDs must contain only numbers.",
            )
            .await?;
            return Ok(());
        };
        if values[..3].iter().any(Option::is_none)
            || values.iter().flatten().any(|value| *value == 0)
        {
            respond_modal(ctx, &response.interaction, "❌ Category, support role, and log channel IDs are required; IDs must be positive.").await?;
            return Ok(());
        }
        let mut config = current.unwrap_or_else(|| default_config(guild_id));
        config.category_id = values[0];
        config.support_role_id = values[1];
        config.log_channel_id = values[2];
        config.block_role_id = values[3];
        super::database::save_config(&ctx.data().tickets_db, &config).await?;
        respond_modal(ctx, &response.interaction, "✅ Ticket routing saved.").await?;
    }
    Ok(())
}

async fn configure_behavior(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
) -> Result<(), Error> {
    let current = super::database::get_config(&ctx.data().tickets_db, guild_id)
        .await?
        .unwrap_or_else(|| default_config(guild_id));
    let modal = CreateQuickModal::new("Ticket Behavior & Limits")
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Max Open Tickets Per User",
                "Use a positive number",
            )
            .value(current.max_user_tickets.to_string())
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Paragraph,
                "Ticket Welcome Message",
                "Use {user} to mention the ticket creator",
            )
            .value(current.ticket_welcome_msg.clone())
            .required(true),
        );
    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        if response.inputs.len() < 2 {
            return Ok(());
        }
        let Ok(max) = response.inputs[0].trim().parse::<i64>() else {
            respond_modal(
                ctx,
                &response.interaction,
                "❌ Max tickets must be a positive number.",
            )
            .await?;
            return Ok(());
        };
        if max <= 0 {
            respond_modal(
                ctx,
                &response.interaction,
                "❌ The ticket limit must be a positive number.",
            )
            .await?;
            return Ok(());
        }
        let mut config = current;
        config.max_user_tickets = max;
        config.ticket_welcome_msg = response.inputs[1].clone();
        super::database::save_config(&ctx.data().tickets_db, &config).await?;
        respond_modal(ctx, &response.interaction, "✅ Ticket behavior saved.").await?;
    }
    Ok(())
}

async fn configure_embed(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
) -> Result<(), Error> {
    let current = super::database::get_config(&ctx.data().tickets_db, guild_id)
        .await?
        .unwrap_or_else(|| default_config(guild_id));
    let modal = CreateQuickModal::new("Customize Ticket Panel")
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Panel Title",
                "Title shown above the ticket buttons",
            )
            .value(current.panel_title.clone())
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Paragraph,
                "Panel Description",
                "Instructions shown to users",
            )
            .value(current.panel_description.clone())
            .required(true),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "Panel Footer", "Footer text")
                .value(current.panel_footer.clone())
                .required(true),
        );
    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        if response.inputs.len() < 3 {
            return Ok(());
        }
        let mut config = current;
        config.panel_title = response.inputs[0].clone();
        config.panel_description = response.inputs[1].clone();
        config.panel_footer = response.inputs[2].clone();
        super::database::save_config(&ctx.data().tickets_db, &config).await?;
        respond_modal(
            ctx,
            &response.interaction,
            "✅ Ticket panel appearance saved.",
        )
        .await?;
    }
    Ok(())
}

async fn add_option(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Add Ticket Category")
        .timeout(PANEL_TIMEOUT)
        .short_field("Category name")
        .field(
            CreateInputText::new(InputTextStyle::Short, "Emoji (optional)", "For example: 🛡️")
                .required(false),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Short description (optional)",
                "Shown to users in the panel",
            )
            .required(false),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Staff Role Override ID (optional)",
                "Uses the default staff role if empty",
            )
            .required(false),
        );
    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        if response.inputs.len() < 4 {
            return Ok(());
        }
        let label = response.inputs[0].trim();
        if label.is_empty() || label.chars().count() > 80 {
            respond_modal(
                ctx,
                &response.interaction,
                "❌ Category names must be 1 to 80 characters.",
            )
            .await?;
            return Ok(());
        }
        let role = if response.inputs[3].trim().is_empty() {
            None
        } else {
            match response.inputs[3].trim().parse::<u64>() {
                Ok(id) if id > 0 => Some(id),
                _ => {
                    respond_modal(
                        ctx,
                        &response.interaction,
                        "❌ Staff role ID must be a positive number.",
                    )
                    .await?;
                    return Ok(());
                }
            }
        };
        let count = super::database::list_options(&ctx.data().tickets_db, guild_id)
            .await?
            .len();
        if count >= 25 {
            respond_modal(
                ctx,
                &response.interaction,
                "❌ Discord supports up to 25 ticket categories per panel.",
            )
            .await?;
            return Ok(());
        }
        super::database::add_option(
            &ctx.data().tickets_db,
            guild_id,
            label,
            optional(&response.inputs[1]),
            optional(&response.inputs[2]),
            role,
        )
        .await?;
        respond_modal(ctx, &response.interaction, "✅ Ticket category added.").await?;
    }
    Ok(())
}

async fn delete_option(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
) -> Result<(), Error> {
    let options = super::database::list_options(&ctx.data().tickets_db, guild_id).await?;
    if options.is_empty() {
        return respond_component(
            ctx,
            interaction,
            "There are no custom ticket categories to delete.",
        )
        .await;
    }
    let modal = CreateQuickModal::new("Delete Ticket Category")
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Category ID",
                "Choose an ID listed in the dashboard",
            )
            .required(true),
        );
    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        let Some(id) = response
            .inputs
            .first()
            .and_then(|value| value.trim().parse::<i64>().ok())
        else {
            respond_modal(ctx, &response.interaction, "❌ Enter a valid category ID.").await?;
            return Ok(());
        };
        let deleted = super::database::delete_option(&ctx.data().tickets_db, guild_id, id).await?;
        respond_modal(
            ctx,
            &response.interaction,
            if deleted > 0 {
                "🗑️ Category deleted. Deploy a new panel to update its buttons."
            } else {
                "❌ That category was not found in this server."
            },
        )
        .await?;
    }
    Ok(())
}

async fn deploy_panel(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: u64,
) -> Result<(), Error> {
    let Some(config) = super::database::get_config(&ctx.data().tickets_db, guild_id).await? else {
        return respond_component(ctx, interaction, "❌ Configure ticket routing first.").await;
    };
    let (Some(category_id), Some(role_id)) = (config.category_id, config.support_role_id) else {
        return respond_component(
            ctx,
            interaction,
            "❌ Configure a ticket category and support role first.",
        )
        .await;
    };
    let guild = ctx.partial_guild().await.ok_or("Missing guild")?;
    let channels = guild.channels(ctx.http()).await?;
    if !channels
        .get(&serenity::ChannelId::new(category_id))
        .is_some_and(|channel| channel.kind == serenity::ChannelType::Category)
    {
        return respond_component(
            ctx,
            interaction,
            "❌ The configured ticket location is not a category in this server.",
        )
        .await;
    }
    if !guild.roles.contains_key(&RoleId::new(role_id)) {
        return respond_component(
            ctx,
            interaction,
            "❌ The configured support role was not found.",
        )
        .await;
    }
    let options = super::database::list_options(&ctx.data().tickets_db, guild_id).await?;
    if options.iter().any(|option| {
        option
            .support_role_id
            .is_some_and(|id| !guild.roles.contains_key(&RoleId::new(id)))
    }) {
        return respond_component(ctx, interaction, "❌ A category has an invalid staff override role. Update or delete that category before deploying the panel.").await;
    }
    let embed = CreateEmbed::new()
        .title(config.panel_title)
        .description(config.panel_description)
        .color(0x5865F2)
        .footer(serenity::CreateEmbedFooter::new(config.panel_footer));
    let components = panel_components(guild_id, &options);
    ctx.channel_id()
        .send_message(
            ctx,
            CreateMessage::new().embed(embed).components(components),
        )
        .await?;
    respond_component(
        ctx,
        interaction,
        "✅ Ticket panel deployed in this channel.",
    )
    .await
}

fn panel_components(
    guild_id: u64,
    options: &[super::database::TicketOption],
) -> Vec<CreateActionRow> {
    if options.is_empty() {
        return vec![CreateActionRow::Buttons(vec![CreateButton::new(format!(
            "ticket_panel:create:{guild_id}"
        ))
        .label("Create Ticket")
        .emoji('📩')
        .style(ButtonStyle::Primary)])];
    }
    options
        .chunks(5)
        .map(|chunk| {
            CreateActionRow::Buttons(
                chunk
                    .iter()
                    .map(|option| {
                        let button = CreateButton::new(format!(
                            "ticket_panel:create:{guild_id}:{}",
                            option.id
                        ))
                        .label(option.label.chars().take(80).collect::<String>())
                        .style(ButtonStyle::Primary);
                        match option.emoji.as_deref().filter(|emoji| !emoji.is_empty()) {
                            Some(emoji) => button.emoji(ReactionType::Unicode(emoji.to_string())),
                            None => button,
                        }
                    })
                    .collect(),
            )
        })
        .collect()
}

#[poise::command(slash_command, required_permissions = "MANAGE_MESSAGES")]
pub async fn open(
    ctx: Context<'_>,
    #[description = "Optional ticket owner"] user: Option<UserId>,
    #[description = "Additional role with access"] role: Option<RoleId>,
    #[description = "Ticket topic"] topic: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx
        .guild_id()
        .ok_or("This command can only be used inside a server")?;
    let Some(config) = super::database::get_config(&ctx.data().tickets_db, guild_id.get()).await?
    else {
        ctx.say("❌ Ticketing system is not configured.").await?;
        return Ok(());
    };
    let (Some(category_id), Some(support_role_id)) = (config.category_id, config.support_role_id)
    else {
        ctx.say("❌ Ticketing system is not configured.").await?;
        return Ok(());
    };
    let number =
        super::database::next_ticket_number(&ctx.data().tickets_db, guild_id.get()).await?;
    let label = topic
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("manual");
    let opener = ctx.author().id;
    let channel_name = manual_channel_name(label, number);
    let channel = super::interactions::create_ticket_channel(
        ctx.serenity_context(),
        super::interactions::TicketChannelSpec {
            guild_id,
            category_id,
            channel_name: &channel_name,
            opener,
            support_role_id,
            additional_member: user,
            additional_role: role.map(|id| id.get()),
        },
    )
    .await?;
    let creator_id = user.unwrap_or(opener);
    super::database::add_ticket(
        &ctx.data().tickets_db,
        channel.id.get(),
        guild_id.get(),
        creator_id.get(),
        support_role_id,
    )
    .await?;
    let mut pings = vec![
        opener.mention().to_string(),
        format!("<@&{support_role_id}>"),
    ];
    if let Some(user) = user.filter(|id| *id != opener) {
        pings.push(user.mention().to_string());
    }
    if let Some(role) = role.filter(|id| id.get() != support_role_id) {
        pings.push(role.mention().to_string());
    }
    let mut description = format!("**Opened By:** {}\n", opener.mention());
    if let Some(user) = user {
        description.push_str(&format!("**User:** {}\n", user.mention()));
    }
    if let Some(role) = role {
        description.push_str(&format!("**Role:** {}\n", role.mention()));
    }
    if let Some(topic) = topic.as_deref() {
        description.push_str(&format!("**Topic:** {topic}\n"));
    }
    description.push('\n');
    description.push_str(
        &config
            .ticket_welcome_msg
            .replace("{user}", &creator_id.mention().to_string()),
    );
    let guild_name = ctx
        .partial_guild()
        .await
        .map(|guild| guild.name)
        .unwrap_or_else(|| "Server".into());
    let embed = super::interactions::ticket_welcome_embed(&guild_name, number, &description)
        .title(format!("🎫 Ticket #{number:04} (Manual)"));
    channel
        .send_message(
            ctx,
            CreateMessage::new()
                .content(pings.join(" | "))
                .embed(embed)
                .components(super::interactions::ticket_controls(None)),
        )
        .await?;
    ctx.say(format!("✅ Ticket opened! Go to {}", channel.mention()))
        .await?;
    Ok(())
}

#[poise::command(slash_command, required_permissions = "MANAGE_MESSAGES")]
pub async fn add(
    ctx: Context<'_>,
    #[description = "User to add to this ticket"] user: UserId,
) -> Result<(), Error> {
    update_ticket_member(ctx, user, true).await
}

#[poise::command(slash_command, required_permissions = "MANAGE_MESSAGES")]
pub async fn remove(
    ctx: Context<'_>,
    #[description = "User to remove from this ticket"] user: UserId,
) -> Result<(), Error> {
    update_ticket_member(ctx, user, false).await
}

async fn update_ticket_member(ctx: Context<'_>, user: UserId, add: bool) -> Result<(), Error> {
    let Some(ticket) =
        super::database::get_ticket(&ctx.data().tickets_db, ctx.channel_id().get()).await?
    else {
        ctx.say("❌ This command can only be used in an active ticket.")
            .await?;
        return Ok(());
    };
    if ticket.status != "open" {
        ctx.say("❌ This command can only be used in an active ticket.")
            .await?;
        return Ok(());
    }
    if add {
        ctx.channel_id()
            .create_permission(
                ctx,
                PermissionOverwrite {
                    allow: ticket_member_permissions(),
                    deny: Permissions::empty(),
                    kind: PermissionOverwriteType::Member(user),
                },
            )
            .await?;
        ctx.say(format!("✅ Added {} to the ticket.", user.mention()))
            .await?;
    } else {
        ctx.channel_id()
            .delete_permission(ctx, PermissionOverwriteType::Member(user))
            .await?;
        ctx.say(format!("✅ Removed {} from the ticket.", user.mention()))
            .await?;
    }
    Ok(())
}

fn ticket_member_permissions() -> Permissions {
    Permissions::VIEW_CHANNEL
        | Permissions::SEND_MESSAGES
        | Permissions::READ_MESSAGE_HISTORY
        | Permissions::ATTACH_FILES
}

#[poise::command(slash_command, required_permissions = "MANAGE_MESSAGES")]
pub async fn rename(
    ctx: Context<'_>,
    #[description = "New ticket channel name"] name: String,
) -> Result<(), Error> {
    let Some(ticket) =
        super::database::get_ticket(&ctx.data().tickets_db, ctx.channel_id().get()).await?
    else {
        ctx.say("❌ This command can only be used in an active ticket.")
            .await?;
        return Ok(());
    };
    if ticket.status != "open" {
        ctx.say("❌ This command can only be used in an active ticket.")
            .await?;
        return Ok(());
    }
    let clean = name
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == ' ' || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if clean.is_empty() {
        ctx.say("❌ Channel names must include at least one letter or number.")
            .await?;
        return Ok(());
    }
    ctx.channel_id()
        .edit(ctx, EditChannel::new().name(&clean))
        .await?;
    ctx.say(format!("✅ Channel renamed to `#{clean}`."))
        .await?;
    Ok(())
}

fn default_config(guild_id: u64) -> super::database::TicketConfig {
    super::database::TicketConfig { guild_id, category_id: None, support_role_id: None, log_channel_id: None, ticket_count: 0, block_role_id: None, panel_title: "📩 Open a Support Ticket".into(), panel_description: "Need assistance or have a question for our support team? Select a category or click below to open a private ticket.".into(), panel_footer: "Support Network".into(), max_user_tickets: 1, ticket_welcome_msg: "Welcome {user}! Please explain your request or issue below in detail.".into() }
}

fn manual_channel_name(topic: &str, number: i64) -> String {
    let clean = topic
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == ' ' || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    let label = if clean.is_empty() { "manual" } else { &clean };
    let suffix = format!("-{number:04}");
    format!(
        "ticket-{}{suffix}",
        label
            .chars()
            .take(100usize.saturating_sub(7 + suffix.len()))
            .collect::<String>()
    )
}

fn mention_channel(id: Option<u64>) -> String {
    id.map(|id| format!("<#{id}>"))
        .unwrap_or_else(|| "Not configured".into())
}
fn mention_role(id: Option<u64>) -> String {
    id.map(|id| format!("<@&{id}>"))
        .unwrap_or_else(|| "Not configured".into())
}
fn optional(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}
fn truncate(value: &str, length: usize) -> String {
    let mut result: String = value.chars().take(length).collect();
    if value.chars().count() > length {
        result.push_str("...");
    }
    result
}

async fn respond_component(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    content: &str,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx.serenity_context(),
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
    ctx: Context<'_>,
    interaction: &serenity::all::ModalInteraction,
    content: &str,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}
