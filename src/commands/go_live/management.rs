use crate::{Context, Error};
use futures::{stream::FuturesUnordered, StreamExt};
use serenity::{
    all::{
        ButtonStyle, ChannelId, CreateActionRow, CreateButton, CreateInteractionResponse,
        CreateInteractionResponseMessage, CreateMessage, GuildId, RoleId,
    },
    collector::ComponentInteractionCollector,
    utils::CreateQuickModal,
};
use sqlx::Row;
use std::time::Duration;

const PANEL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CREATORS_IN_PANEL: usize = 20;

/// /golive manage
#[poise::command(
    slash_command,
    rename = "manage",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn manage(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let Some(guild_id) = ctx.guild_id() else {
        ctx.say("❌ This command can only be used inside a server.")
            .await?;
        return Ok(());
    };

    let guild_id = guild_id.get() as i64;

    let rows = sqlx::query(
        "SELECT id, name, twitch_username, is_enabled, is_currently_live
         FROM go_live_creators
         WHERE guild_id = ?
         ORDER BY name COLLATE NOCASE",
    )
    .bind(guild_id)
    .fetch_all(&ctx.data().stats_db)
    .await?;

    let mut description = String::from("🔴 **GoLive Management**\n\n");

    if rows.is_empty() {
        description.push_str("No live creators configured for this server yet.\n");
    } else {
        description.push_str("**Configured creators:**\n\n");

        for row in rows.iter().take(MAX_CREATORS_IN_PANEL) {
            let id: i64 = row.try_get("id")?;
            let name: Option<String> = row.try_get("name")?;
            let twitch: Option<String> = row.try_get("twitch_username")?;
            let enabled: Option<i64> = row.try_get("is_enabled")?;
            let is_live: Option<i64> = row.try_get("is_currently_live")?;

            let name = name.unwrap_or_else(|| "Unnamed Creator".to_string());
            let twitch = twitch.unwrap_or_else(|| "Not configured".to_string());

            let status = if enabled.unwrap_or(1) == 0 {
                "⏸️ Disabled"
            } else if is_live.unwrap_or(0) != 0 {
                "🔴 LIVE"
            } else {
                "⚫ Offline"
            };

            description.push_str(&format!(
                "• **{name}** — `{twitch}` — {status} — ID: `{id}`\n"
            ));
        }

        if rows.len() > MAX_CREATORS_IN_PANEL {
            description.push_str("\nShowing the first 20 creators.");
        }
    }

    description.push_str("\n\nUse **Add Creator** to configure another Twitch creator.");

    let mut components = Vec::new();
    let mut buttons = Vec::new();

    for row in rows.iter().take(MAX_CREATORS_IN_PANEL) {
        let id: i64 = row.try_get("id")?;
        let name: Option<String> = row.try_get("name")?;

        let label = name
            .unwrap_or_else(|| "Unnamed Creator".to_string())
            .chars()
            .take(80)
            .collect::<String>();

        buttons.push(
            CreateButton::new(format!("golive_select:{id}"))
                .label(label)
                .style(ButtonStyle::Secondary),
        );

        if buttons.len() == 5 {
            components.push(CreateActionRow::Buttons(buttons));
            buttons = Vec::new();
        }
    }

    buttons.push(
        CreateButton::new("golive_add")
            .label("Add Creator")
            .style(ButtonStyle::Success),
    );

    if !buttons.is_empty() {
        components.push(CreateActionRow::Buttons(buttons));
    }

    ctx.send(
        poise::CreateReply::default()
            .content(description)
            .components(components)
            .ephemeral(true),
    )
    .await?;

    let collector = ComponentInteractionCollector::new(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .filter(|interaction| {
            interaction.data.custom_id == "golive_add"
                || interaction.data.custom_id.starts_with("golive_select:")
        })
        .timeout(PANEL_TIMEOUT);

    let mut interactions = Box::pin(collector.stream());
    let mut handlers = FuturesUnordered::new();

    loop {
        tokio::select! {
            maybe_interaction = interactions.next() => {
                let Some(interaction) = maybe_interaction else {
                    break;
                };

                handlers.push(async move {
                    println!("👆 GoLive dashboard interaction: {}", interaction.data.custom_id);

                    let custom_id = interaction.data.custom_id.clone();

                    if custom_id == "golive_add" {
                        add_creator(ctx, &interaction, guild_id).await?;
                    } else if let Some(id) = custom_id.strip_prefix("golive_select:") {
                        let creator_id: i64 = id.parse()?;

                        select_creator(ctx, &interaction, creator_id, guild_id).await?;
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

/// Open the Add Creator modal and insert a guild-scoped creator.
async fn add_creator(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    guild_id: i64,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Add Live Creator")
        .timeout(PANEL_TIMEOUT)
        .short_field("Creator name")
        .short_field("Twitch username");

    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        let inputs = response.inputs;

        if inputs.len() < 2 {
            return Ok(());
        }

        let name = inputs[0].trim().to_string();
        let username = inputs[1].trim().trim_start_matches('@').to_string();

        if name.is_empty() {
            respond(
                ctx,
                &response.interaction,
                "❌ Creator name cannot be empty.",
            )
            .await?;

            return Ok(());
        }

        if username.is_empty() {
            respond(
                ctx,
                &response.interaction,
                "❌ Twitch username cannot be empty.",
            )
            .await?;

            return Ok(());
        }

        let result = sqlx::query(
            "INSERT INTO go_live_creators (
                guild_id,
                name,
                twitch_username,
                is_enabled,
                is_currently_live
            )
            VALUES (?, ?, ?, 1, 0)",
        )
        .bind(guild_id)
        .bind(&name)
        .bind(&username)
        .execute(&ctx.data().stats_db)
        .await;

        match result {
            Ok(result) => {
                respond(
                    ctx,
                    &response.interaction,
                    &format!(
                        "✅ Added **{name}** with Twitch username `{username}`!\nCreator ID: `{}`",
                        result.last_insert_rowid()
                    ),
                )
                .await?;
            }
            Err(error) => {
                tracing::error!("Failed to add GoLive creator: {error}");

                respond(
                    ctx,
                    &response.interaction,
                    "❌ Failed to add the creator. Check the bot logs.",
                )
                .await?;
            }
        }
    }

    Ok(())
}

/// Validate a selected creator and show its management buttons.
async fn select_creator(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT name, twitch_username, is_enabled, is_currently_live
         FROM go_live_creators
         WHERE id = ? AND guild_id = ?",
    )
    .bind(creator_id)
    .bind(guild_id)
    .fetch_optional(&ctx.data().stats_db)
    .await?;

    let Some(row) = row else {
        respond_component(
            ctx,
            interaction,
            "❌ That creator wasn't found in this server.",
        )
        .await?;
        return Ok(());
    };

    let name: Option<String> = row.try_get("name")?;
    let username: Option<String> = row.try_get("twitch_username")?;
    let enabled: Option<i64> = row.try_get("is_enabled")?;
    let is_live: Option<i64> = row.try_get("is_currently_live")?;

    let status = if enabled.unwrap_or(1) == 0 {
        "⏸️ Disabled"
    } else if is_live.unwrap_or(0) != 0 {
        "🔴 LIVE"
    } else {
        "⚫ Offline"
    };

    let content = format!(
        "🔴 **GoLive Creator Management**\n\n**{}**\nTwitch: `{}`\nStatus: {}",
        name.unwrap_or_else(|| "Unnamed Creator".to_string()),
        username.unwrap_or_else(|| "Not configured".to_string()),
        status
    );

    let buttons = vec![
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("golive_channel:{creator_id}"))
                .label("Set Alert Channel")
                .style(ButtonStyle::Primary),
            CreateButton::new(format!("golive_role:{creator_id}"))
                .label("Set Role Ping")
                .style(ButtonStyle::Primary),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("golive_username:{creator_id}"))
                .label("Edit Twitch Username")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("golive_message:{creator_id}"))
                .label("Edit Alert Message")
                .style(ButtonStyle::Secondary),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("golive_toggle:{creator_id}"))
                .label("Toggle Alerts")
                .style(ButtonStyle::Success),
            CreateButton::new(format!("golive_test:{creator_id}"))
                .label("Send Test Alert")
                .style(ButtonStyle::Success),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("golive_delete:{creator_id}"))
                .label("Delete Creator")
                .style(ButtonStyle::Danger),
            CreateButton::new(format!("golive_back:{creator_id}"))
                .label("Back to Menu")
                .style(ButtonStyle::Secondary),
        ]),
    ];

    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .components(buttons)
                    .ephemeral(true),
            ),
        )
        .await?;

    creator_panel(ctx, creator_id, guild_id).await
}

/// Listen for interactions on an individual creator panel.
async fn creator_panel(ctx: Context<'_>, creator_id: i64, guild_id: i64) -> Result<(), Error> {
    let button_ids = [
        format!("golive_channel:{creator_id}"),
        format!("golive_role:{creator_id}"),
        format!("golive_username:{creator_id}"),
        format!("golive_message:{creator_id}"),
        format!("golive_toggle:{creator_id}"),
        format!("golive_test:{creator_id}"),
        format!("golive_delete:{creator_id}"),
        format!("golive_back:{creator_id}"),
    ];

    let collector = ComponentInteractionCollector::new(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .filter(move |interaction| {
            button_ids
                .iter()
                .any(|id| id == &interaction.data.custom_id)
        })
        .timeout(PANEL_TIMEOUT);

    let mut interactions = Box::pin(collector.stream());
    let mut handlers = FuturesUnordered::new();

    loop {
        tokio::select! {
            maybe_interaction = interactions.next() => {
                let Some(interaction) = maybe_interaction else {
                    break;
                };

                // Return to the existing creator list.
                if interaction.data.custom_id == format!("golive_back:{creator_id}") {
                    interaction
                        .create_response(
                            ctx.serenity_context(),
                            CreateInteractionResponse::UpdateMessage(
                                CreateInteractionResponseMessage::new()
                                    .content("↩️ Returned to the GoLive creator list.")
                                    .components(vec![]),
                            ),
                        )
                        .await?;

                    break;
                }

                handlers.push(async move {
                    let custom_id = interaction.data.custom_id.clone();

                    match custom_id.as_str() {
                        id if id.starts_with("golive_channel:") => {
                            set_alert_channel(ctx, &interaction, creator_id, guild_id).await?;
                        }
                        id if id.starts_with("golive_role:") => {
                            set_alert_role(ctx, &interaction, creator_id, guild_id).await?;
                        }
                        id if id.starts_with("golive_username:") => {
                            edit_twitch_username(ctx, &interaction, creator_id, guild_id).await?;
                        }
                        id if id.starts_with("golive_message:") => {
                            edit_alert_message(ctx, &interaction, creator_id, guild_id).await?;
                        }
                        id if id.starts_with("golive_toggle:") => {
                            toggle_alerts(ctx, &interaction, creator_id, guild_id).await?;
                        }
                        id if id.starts_with("golive_test:") => {
                            send_test_alert(ctx, &interaction, creator_id, guild_id).await?;
                        }
                        id if id.starts_with("golive_delete:") => {
                            confirm_delete_creator(ctx, &interaction, creator_id, guild_id).await?;
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

/// Ask for confirmation before deleting a creator.
async fn confirm_delete_creator(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let confirm_id = format!("golive_delete_confirm:{creator_id}");
    let cancel_id = format!("golive_delete_cancel:{creator_id}");

    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("⚠️ **Are you sure you want to delete this creator?**\nThis cannot be undone.")
                    .components(vec![CreateActionRow::Buttons(vec![
                        CreateButton::new(&confirm_id)
                            .label("Confirm Delete")
                            .style(ButtonStyle::Danger),
                        CreateButton::new(&cancel_id)
                            .label("Cancel")
                            .style(ButtonStyle::Secondary),
                    ])])
                    .ephemeral(true),
            ),
        )
        .await?;

    let confirm_filter = confirm_id.clone();
    let cancel_filter = cancel_id.clone();

    let collector = ComponentInteractionCollector::new(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .filter(move |component| {
            component.data.custom_id == confirm_filter || component.data.custom_id == cancel_filter
        })
        .timeout(PANEL_TIMEOUT);

    let Some(response) = collector.await else {
        return Ok(());
    };

    if response.data.custom_id == cancel_id {
        respond_component(ctx, &response, "Deletion cancelled.").await?;
        return Ok(());
    }

    let result = sqlx::query(
        "DELETE FROM go_live_creators
         WHERE id = ? AND guild_id = ?",
    )
    .bind(creator_id)
    .bind(guild_id)
    .execute(&ctx.data().stats_db)
    .await?;

    if result.rows_affected() == 0 {
        respond_component(
            ctx,
            &response,
            "❌ Creator not found in this server. Nothing was deleted.",
        )
        .await?;
    } else {
        respond_component(ctx, &response, "🗑️ Creator successfully deleted.").await?;
    }

    Ok(())
}

/// Send a test alert using the creator's configured message and role.
async fn send_test_alert(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT name, twitch_username, live_alert_channel_id,
                live_alert_role_id, custom_message
         FROM go_live_creators
         WHERE id = ? AND guild_id = ?",
    )
    .bind(creator_id)
    .bind(guild_id)
    .fetch_optional(&ctx.data().stats_db)
    .await?;

    let Some(row) = row else {
        respond_component(ctx, interaction, "❌ Creator not found in this server.").await?;
        return Ok(());
    };

    let name: String = row.try_get("name")?;
    let username: String = row.try_get("twitch_username")?;
    let channel_id: Option<i64> = row.try_get("live_alert_channel_id")?;
    let role_id: Option<i64> = row.try_get("live_alert_role_id")?;
    let custom_message: Option<String> = row.try_get("custom_message")?;

    let Some(channel_id) = channel_id.and_then(|id| u64::try_from(id).ok()) else {
        respond_component(
            ctx,
            interaction,
            "❌ No valid alert channel is configured. Set one first.",
        )
        .await?;
        return Ok(());
    };

    let message = super::polling::format_alert_message(
        custom_message.as_deref(),
        &name,
        &username,
        Some("This is a test stream title"),
        Some("Test Category"),
        role_id,
    );

    match ChannelId::new(channel_id)
        .send_message(
            ctx.serenity_context(),
            CreateMessage::new().content(message),
        )
        .await
    {
        Ok(_) => {
            respond_component(
                ctx,
                interaction,
                "✅ Test alert sent to the configured channel.",
            )
            .await?;
        }
        Err(error) => {
            eprintln!("[GoLive] Failed to send test alert for creator {creator_id}: {error}");

            respond_component(
                ctx,
                interaction,
                "❌ Failed to send the test alert. Check the bot's channel permissions.",
            )
            .await?;
        }
    }

    Ok(())
}

/// Enable or disable live alerts for a creator.
async fn toggle_alerts(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let enabled: Option<i64> = sqlx::query_scalar(
        "UPDATE go_live_creators
         SET is_enabled = CASE WHEN is_enabled = 1 THEN 0 ELSE 1 END
         WHERE id = ? AND guild_id = ?
         RETURNING is_enabled",
    )
    .bind(creator_id)
    .bind(guild_id)
    .fetch_optional(&ctx.data().stats_db)
    .await?;

    let Some(enabled) = enabled else {
        respond_component(ctx, interaction, "❌ Creator not found in this server.").await?;
        return Ok(());
    };

    let message = if enabled != 0 {
        "✅ Live alerts are now **enabled** for this creator."
    } else {
        "🔕 Live alerts are now **disabled** for this creator."
    };

    respond_component(ctx, interaction, message).await?;

    Ok(())
}

/// Set or reset a creator's custom live alert message.
async fn edit_alert_message(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Edit Alert Message")
        .timeout(PANEL_TIMEOUT)
        .paragraph_field("Custom message (blank = default)");

    let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    else {
        return Ok(());
    };

    let message = response
        .inputs
        .first()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let result = sqlx::query(
        "UPDATE go_live_creators
         SET custom_message = ?
         WHERE id = ? AND guild_id = ?",
    )
    .bind(&message)
    .bind(creator_id)
    .bind(guild_id)
    .execute(&ctx.data().stats_db)
    .await?;

    if result.rows_affected() == 0 {
        respond(
            ctx,
            &response.interaction,
            "❌ Creator not found in this server.",
        )
        .await?;
        return Ok(());
    }

    let confirmation = if message.is_some() {
        "✅ Custom alert message saved."
    } else {
        "✅ Custom alert message cleared. The default message will be used."
    };

    respond(ctx, &response.interaction, confirmation).await?;

    Ok(())
}

/// Update a creator's Twitch username and clear its cached Twitch ID.
async fn edit_twitch_username(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Edit Twitch Username")
        .timeout(PANEL_TIMEOUT)
        .short_field("New Twitch username");

    let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    else {
        return Ok(());
    };

    let username = response
        .inputs
        .first()
        .map(|s| s.trim().trim_start_matches('@').to_string())
        .unwrap_or_default();

    if username.is_empty() {
        respond(
            ctx,
            &response.interaction,
            "❌ Twitch username cannot be empty.",
        )
        .await?;
        return Ok(());
    }

    let result = sqlx::query(
        "UPDATE go_live_creators
         SET twitch_username = ?,
             twitch_user_id = NULL,
             is_currently_live = 0
         WHERE id = ? AND guild_id = ?",
    )
    .bind(&username)
    .bind(creator_id)
    .bind(guild_id)
    .execute(&ctx.data().stats_db)
    .await?;

    if result.rows_affected() == 0 {
        respond(
            ctx,
            &response.interaction,
            "❌ Creator not found in this server.",
        )
        .await?;
        return Ok(());
    }

    respond(
        ctx,
        &response.interaction,
        &format!(
            "✅ Twitch username updated to `{username}`.\nThe cached Twitch account ID has been cleared and will be resolved during the next poll."
        ),
    )
    .await?;

    Ok(())
}

/// Configure or clear the role ping for a creator.
async fn set_alert_role(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Set Role Ping")
        .timeout(PANEL_TIMEOUT)
        .short_field("Role ID (blank = no role ping)");

    let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    else {
        return Ok(());
    };

    let input = response.inputs.first().map(|s| s.trim()).unwrap_or("");

    // An empty field clears the configured role.
    let role_id = if input.is_empty() {
        None
    } else {
        let id = match input.parse::<u64>() {
            Ok(id) => id,
            Err(_) => {
                respond(
                    ctx,
                    &response.interaction,
                    "❌ Please enter a valid Discord role ID.",
                )
                .await?;
                return Ok(());
            }
        };

        let guild = GuildId::new(guild_id as u64);
        let roles = guild.roles(ctx.serenity_context()).await?;
        let role = RoleId::new(id);

        if !roles.contains_key(&role) {
            respond(
                ctx,
                &response.interaction,
                "❌ That role doesn't belong to this server.",
            )
            .await?;
            return Ok(());
        }

        Some(id)
    };

    let result = sqlx::query(
        "UPDATE go_live_creators
         SET live_alert_role_id = ?
         WHERE id = ? AND guild_id = ?",
    )
    .bind(role_id.map(|id| id as i64))
    .bind(creator_id)
    .bind(guild_id)
    .execute(&ctx.data().stats_db)
    .await?;

    if result.rows_affected() == 0 {
        respond(
            ctx,
            &response.interaction,
            "❌ Creator not found in this server.",
        )
        .await?;
        return Ok(());
    }

    let message = match role_id {
        Some(id) => format!("✅ Role ping updated to <@&{id}>."),
        None => "✅ Role ping cleared. No role will be pinged.".to_string(),
    };

    respond(ctx, &response.interaction, &message).await?;

    Ok(())
}

/// Configure the alert channel for a creator.
async fn set_alert_channel(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
    guild_id: i64,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Set Alert Channel")
        .timeout(PANEL_TIMEOUT)
        .field(
            serenity::all::CreateInputText::new(
                serenity::all::InputTextStyle::Short,
                "Channel ID (blank = current channel)",
                "channel_id",
            )
            .required(false),
        );

    let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    else {
        return Ok(());
    };

    let input = response.inputs.first().map(|s| s.trim()).unwrap_or("");

    let channel_id = if input.is_empty() {
        response.interaction.channel_id.get()
    } else {
        match input.parse::<u64>() {
            Ok(id) => id,
            Err(_) => {
                respond(
                    ctx,
                    &response.interaction,
                    "❌ Please enter a valid Discord channel ID.",
                )
                .await?;
                return Ok(());
            }
        }
    };

    let channel = ChannelId::new(channel_id);
    let guild = GuildId::new(guild_id as u64);

    let channels = guild.channels(ctx.serenity_context()).await?;

    if !channels.contains_key(&channel) {
        respond(
            ctx,
            &response.interaction,
            "❌ That channel doesn't belong to this server.",
        )
        .await?;
        return Ok(());
    }

    let result = sqlx::query(
        "UPDATE go_live_creators
         SET live_alert_channel_id = ?
         WHERE id = ? AND guild_id = ?",
    )
    .bind(channel_id as i64)
    .bind(creator_id)
    .bind(guild_id)
    .execute(&ctx.data().stats_db)
    .await?;

    if result.rows_affected() == 0 {
        respond(
            ctx,
            &response.interaction,
            "❌ Creator not found in this server.",
        )
        .await?;
        return Ok(());
    }

    respond(
        ctx,
        &response.interaction,
        &format!("✅ Alert channel updated to <#{channel_id}>."),
    )
    .await?;

    Ok(())
}

/// Send an ephemeral response to a modal submission.
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

/// Send an ephemeral response to a component interaction.
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

/// /golive
#[poise::command(slash_command, subcommands("manage"))]
pub async fn golive(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}
