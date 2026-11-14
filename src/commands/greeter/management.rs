use crate::{Context, Error};
use futures::{stream::FuturesUnordered, StreamExt};
use serenity::{
    all::{
        ButtonStyle, CreateActionRow, CreateButton, CreateInputText, CreateInteractionResponse,
        CreateInteractionResponseMessage, InputTextStyle,
    },
    utils::CreateQuickModal,
};
use std::time::Duration;

const PANEL_TIMEOUT: Duration = Duration::from_secs(300);

/// Configure welcome and farewell messages for this server.
#[poise::command(
    slash_command,
    rename = "manage",
    required_permissions = "MANAGE_GUILD"
)]
pub async fn manage(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or("This command can only be used inside a server.")?
        .get();

    let config = super::database::get_config(&ctx.data().greeters_db, guild_id).await?;

    let join_channel = config
        .join_channel_id
        .map(|id| format!("<#{id}>"))
        .unwrap_or_else(|| "Not configured".into());

    let leave_channel = config
        .leave_channel_id
        .map(|id| format!("<#{id}>"))
        .unwrap_or_else(|| "Not configured".into());

    let join_status = if config.join_enabled {
        "🟢 Enabled"
    } else {
        "🔴 Disabled"
    };

    let leave_status = if config.leave_enabled {
        "🟢 Enabled"
    } else {
        "🔴 Disabled"
    };

    let description = format!(
        "👋 **Greeter Management — {}**\n\n\
         **Join Messages**\n\
         • Channel: {join_channel}\n\
         • Status: {join_status}\n\
         • Title: {}\n\
         • Description: {}\n\n\
         **Leave Messages**\n\
         • Channel: {leave_channel}\n\
         • Status: {leave_status}\n\
         • Title: {}\n\
         • Description: {}\n\n\
         Use the buttons below to configure your server's greeter.",
        ctx.guild()
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "Server".to_string()),
        config.join_title,
        truncate(&config.join_description, 100),
        config.leave_title,
        truncate(&config.leave_description, 100),
    );

    let components = vec![
        CreateActionRow::Buttons(vec![
            CreateButton::new("greeter_channels")
                .label("Configure Channels")
                .style(ButtonStyle::Primary),
            CreateButton::new("greeter_join")
                .label("Customize Join Message")
                .style(ButtonStyle::Primary),
            CreateButton::new("greeter_leave")
                .label("Customize Leave Message")
                .style(ButtonStyle::Primary),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new("greeter_test_join")
                .label("Test Join Message")
                .style(ButtonStyle::Success),
            CreateButton::new("greeter_test_leave")
                .label("Test Leave Message")
                .style(ButtonStyle::Secondary),
        ]),
    ];

    let reply = ctx
        .send(
            poise::CreateReply::default()
                .content(description)
                .components(components)
                .ephemeral(true),
        )
        .await?;

    let dashboard_message = reply.message().await?;

    let mut interactions = dashboard_message
        .await_component_interaction(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .timeout(PANEL_TIMEOUT)
        .stream();

    let mut handlers = FuturesUnordered::new();

    loop {
        tokio::select! {
            maybe_interaction = interactions.next() => {
                let Some(interaction) = maybe_interaction else {
                    break;
                };

                handlers.push(async move {
                    println!(
                        "👆 Greeter dashboard interaction: {}",
                        interaction.data.custom_id
                    );

                    match interaction.data.custom_id.as_str() {
                        "greeter_channels" => {
                            configure_channels(ctx, &interaction).await?;
                        }
                        "greeter_join" => {
                            customize_message(ctx, &interaction, true).await?;
                        }
                        "greeter_leave" => {
                            customize_message(ctx, &interaction, false).await?;
                        }
                        "greeter_test_join" => {
                            test_message(ctx, &interaction, true).await?;
                        }
                        "greeter_test_leave" => {
                            test_message(ctx, &interaction, false).await?;
                        }
                        _ => {}
                    }

                    Ok::<(), Error>(())
                });
            }

            result = handlers.next(), if !handlers.is_empty() => {
                if let Some(result) = result {
                    result?;
                }
            }
        }
    }

    Ok(())
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut result: String = value.chars().take(max_chars).collect();

    if value.chars().count() > max_chars {
        result.push_str("...");
    }

    result
}

/// Manage automatic member welcome and farewell messages.
#[poise::command(slash_command, subcommands("manage"))]
pub async fn greeter(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

async fn configure_channels(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Missing guild ID")?.get();

    let config = super::database::get_config(&ctx.data().greeters_db, guild_id).await?;

    let modal = CreateQuickModal::new("Configure Greeter Channels")
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(InputTextStyle::Short, "Join channel ID (empty clears)", "")
                .value(
                    config
                        .join_channel_id
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                )
                .required(false),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "Leave channel ID (empty clears)", "")
                .value(
                    config
                        .leave_channel_id
                        .map(|id| id.to_string())
                        .unwrap_or_default(),
                )
                .required(false),
        );

    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        if response.inputs.len() < 2 {
            return Ok(());
        }

        let parse_channel = |value: &str| -> Result<Option<u64>, &'static str> {
            let value = value.trim();

            if value.is_empty() {
                return Ok(None);
            }

            let id = value
                .parse::<u64>()
                .map_err(|_| "Channel IDs must contain only numbers.")?;

            if id == 0 {
                return Err("Channel IDs must be positive.");
            }

            Ok(Some(id))
        };

        let join_channel = match parse_channel(&response.inputs[0]) {
            Ok(value) => value,
            Err(error) => {
                respond(ctx, &response.interaction, error).await?;
                return Ok(());
            }
        };

        let leave_channel = match parse_channel(&response.inputs[1]) {
            Ok(value) => value,
            Err(error) => {
                respond(ctx, &response.interaction, error).await?;
                return Ok(());
            }
        };

        let mut config = config;
        config.join_channel_id = join_channel;
        config.leave_channel_id = leave_channel;

        super::database::save_config(&ctx.data().greeters_db, &config).await?;

        respond(
            ctx,
            &response.interaction,
            "✅ Greeter channels updated successfully!",
        )
        .await?;
    }

    Ok(())
}

async fn customize_message(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    is_join: bool,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Missing guild ID")?.get();

    let config = super::database::get_config(&ctx.data().greeters_db, guild_id).await?;

    let (title, description, footer, color) = if is_join {
        (
            &config.join_title,
            &config.join_description,
            &config.join_footer,
            &config.join_color,
        )
    } else {
        (
            &config.leave_title,
            &config.leave_description,
            &config.leave_footer,
            &config.leave_color,
        )
    };

    let modal_title = if is_join {
        "Customize Join Message"
    } else {
        "Customize Leave Message"
    };

    let modal = CreateQuickModal::new(modal_title)
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(InputTextStyle::Short, "Embed title", "")
                .value(title)
                .required(true),
        )
        .field(
            CreateInputText::new(InputTextStyle::Paragraph, "Embed description", "")
                .value(description)
                .required(true),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "Embed footer", "")
                .value(footer)
                .required(false),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "Embed color (name or hex)", "")
                .value(color)
                .required(false),
        );

    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        if response.inputs.len() < 4 {
            return Ok(());
        }

        let title = response.inputs[0].trim();
        let description = response.inputs[1].trim();
        let footer = response.inputs[2].trim();
        let color = response.inputs[3].trim();

        if title.is_empty() || description.is_empty() {
            respond(
                ctx,
                &response.interaction,
                "❌ The embed title and description cannot be empty.",
            )
            .await?;

            return Ok(());
        }

        if title.chars().count() > 256
            || description.chars().count() > 4000
            || footer.chars().count() > 2048
            || color.chars().count() > 32
        {
            respond(
                ctx,
                &response.interaction,
                "❌ One or more fields exceed Discord's embed limits.",
            )
            .await?;

            return Ok(());
        }

        let mut config = config;

        if is_join {
            config.join_title = title.to_string();
            config.join_description = description.to_string();
            config.join_footer = footer.to_string();

            if !color.is_empty() {
                config.join_color = color.to_string();
            }
        } else {
            config.leave_title = title.to_string();
            config.leave_description = description.to_string();
            config.leave_footer = footer.to_string();

            if !color.is_empty() {
                config.leave_color = color.to_string();
            }
        }

        super::database::save_config(&ctx.data().greeters_db, &config).await?;

        respond(
            ctx,
            &response.interaction,
            "✅ Greeter message customization saved!",
        )
        .await?;
    }

    Ok(())
}

async fn respond(
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

async fn test_message(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    is_join: bool,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Missing guild ID")?.get();

    let config = super::database::get_config(&ctx.data().greeters_db, guild_id).await?;

    let (channel_id, enabled) = if is_join {
        (config.join_channel_id, config.join_enabled)
    } else {
        (config.leave_channel_id, config.leave_enabled)
    };

    let Some(channel_id) = channel_id else {
        interaction
            .create_response(
                ctx.serenity_context(),
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("❌ No channel has been configured for this message.")
                        .ephemeral(true),
                ),
            )
            .await?;

        return Ok(());
    };

    if !enabled {
        interaction
            .create_response(
                ctx.serenity_context(),
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("❌ This message type is currently disabled.")
                        .ephemeral(true),
                ),
            )
            .await?;

        return Ok(());
    }

    let channel = serenity::all::ChannelId::new(channel_id);

    let guild = {
        let guild_ref = ctx.guild().ok_or("Missing guild")?;
        (*guild_ref).clone()
    };

    let embed = super::events::build_test_embed(&config, ctx.author(), &guild, is_join);

    channel
        .send_message(
            ctx.serenity_context(),
            serenity::all::CreateMessage::new()
                .embed(embed)
                .allowed_mentions(serenity::all::CreateAllowedMentions::new()),
        )
        .await?;

    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("✅ Test message sent!")
                    .ephemeral(true),
            ),
        )
        .await?;

    Ok(())
}
