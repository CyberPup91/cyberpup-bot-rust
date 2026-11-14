use crate::{Context, Error};
use futures::{stream::FuturesUnordered, StreamExt};
use serenity::{
    all::{
        ButtonStyle, ChannelId, CreateActionRow, CreateButton, CreateInputText,
        CreateInteractionResponse, CreateInteractionResponseMessage, InputTextStyle, MessageId,
    },
    collector::ComponentInteractionCollector,
    utils::CreateQuickModal,
};
use sqlx::Row;
use std::time::Duration;

const PANEL_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CREATORS_IN_PANEL: usize = 20;

/// Manage creator profiles, tracking settings, and dashboard preferences.
#[poise::command(
    slash_command,
    rename = "manage",
    required_permissions = "ADMINISTRATOR"
)]
pub async fn manage(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let rows = sqlx::query(
        "SELECT id, name, twitch_username
         FROM creators
         ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(&ctx.data().stats_db)
    .await?;

    let mut description = String::from("📊 **Creator Stats Management**\n\n");

    if rows.is_empty() {
        description.push_str("No creators configured yet.\n");
    } else {
        description.push_str("Select a creator below to manage their configuration.\n\n");

        for row in &rows {
            let id: i64 = row.try_get("id")?;
            let name: String = row.try_get("name")?;
            let twitch: Option<String> = row.try_get("twitch_username")?;

            description.push_str(&format!(
                "• **{}** — ID: `{}` | Twitch: {}\n",
                name,
                id,
                twitch.unwrap_or_else(|| "Not configured".into())
            ));
        }
    }

    if rows.len() > MAX_CREATORS_IN_PANEL {
        description.push_str("\nShowing the first 20 creators in the selection controls.");
    }

    description.push_str(
        "\n\n**Add Creator** creates a new entry. \
         Select an existing creator to open its management menu.",
    );

    let mut components = Vec::new();
    let mut creator_buttons = Vec::new();

    for row in rows.iter().take(MAX_CREATORS_IN_PANEL) {
        let id: i64 = row.try_get("id")?;
        let name: String = row.try_get("name")?;

        creator_buttons.push(
            CreateButton::new(format!("creator_select:{id}"))
                .label(name.chars().take(80).collect::<String>())
                .style(ButtonStyle::Primary),
        );
    }

    for chunk in creator_buttons.chunks(5) {
        components.push(CreateActionRow::Buttons(chunk.to_vec()));
    }

    components.push(CreateActionRow::Buttons(vec![CreateButton::new(
        "creator_add",
    )
    .label("Add Creator")
    .style(ButtonStyle::Success)]));

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
            interaction.data.custom_id == "creator_add"
                || interaction.data.custom_id.starts_with("creator_select:")
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
                    let custom_id = interaction.data.custom_id.clone();

                    println!("👆 Creator dashboard interaction: {custom_id}");

                    if custom_id == "creator_add" {
                        add_creator(ctx, &interaction).await?;
                    } else if let Some(id) = custom_id
                        .strip_prefix("creator_select:")
                        .and_then(|value| value.parse::<i64>().ok())
                    {
                        creator_actions(ctx, &interaction, id).await?;
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

/// Display the action menu for a selected creator.
async fn creator_actions(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
) -> Result<(), Error> {
    let row = sqlx::query("SELECT name FROM creators WHERE id = ?")
        .bind(creator_id)
        .fetch_optional(&ctx.data().stats_db)
        .await?;

    let Some(row) = row else {
        respond_component(ctx, interaction, "❌ That creator no longer exists.").await?;

        return Ok(());
    };

    let name: String = row.try_get("name")?;

    let components = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("creator_edit:{creator_id}"))
            .label("Edit Details")
            .style(ButtonStyle::Primary),
        CreateButton::new(format!("creator_settings:{creator_id}"))
            .label("Settings")
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("creator_delete:{creator_id}"))
            .label("Delete Creator")
            .style(ButtonStyle::Danger),
    ])];

    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(format!(
                        "⚙️ **Managing {name}**\n\n\
                         Choose what you'd like to configure."
                    ))
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;

    let collector = ComponentInteractionCollector::new(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .filter(move |interaction| {
            let id = &interaction.data.custom_id;

            id == &format!("creator_edit:{creator_id}")
                || id == &format!("creator_settings:{creator_id}")
                || id == &format!("creator_delete:{creator_id}")
        })
        .timeout(PANEL_TIMEOUT);

    let mut interactions = Box::pin(collector.stream());
    let mut handlers = FuturesUnordered::new();

    loop {
        tokio::select! {
            maybe_action = interactions.next() => {
                let Some(action) = maybe_action else {
                    break;
                };

                handlers.push(async move {
                    let custom_id = action.data.custom_id.clone();

                    println!("👆 Creator action interaction: {custom_id}");

                    match custom_id.as_str() {
                        id if id == format!("creator_edit:{creator_id}") => {
                            edit_creator(ctx, &action, creator_id).await?;
                        }
                        id if id == format!("creator_settings:{creator_id}") => {
                            edit_settings(ctx, &action, creator_id).await?;
                        }
                        id if id == format!("creator_delete:{creator_id}") => {
                            delete_creator(ctx, &action, creator_id).await?;
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

/// Open the Add Creator modal and insert the new creator.
async fn add_creator(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Add Creator")
        .timeout(PANEL_TIMEOUT)
        .short_field("Creator name")
        .short_field("Twitch username (optional)")
        .short_field("YouTube channel ID (optional)")
        .short_field("Discord invite code (optional)");

    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        let inputs = response.inputs;

        if inputs.len() < 4 {
            return Ok(());
        }

        let name = inputs[0].trim().to_string();

        if name.is_empty() {
            respond(
                ctx,
                &response.interaction,
                "❌ Creator name cannot be empty.",
            )
            .await?;

            return Ok(());
        }

        let result = sqlx::query(
            "INSERT INTO creators (
                name,
                twitch_username,
                youtube_id,
                discord_invite_code
            )
            VALUES (?, ?, ?, ?)",
        )
        .bind(&name)
        .bind(optional_input(&inputs[1]))
        .bind(optional_input(&inputs[2]))
        .bind(optional_input(&inputs[3]))
        .execute(&ctx.data().stats_db)
        .await;

        match result {
            Ok(result) => {
                respond(
                    ctx,
                    &response.interaction,
                    &format!(
                        "✅ Added **{}**! Creator ID: `{}`",
                        name,
                        result.last_insert_rowid()
                    ),
                )
                .await?;
            }
            Err(error) => {
                tracing::error!("Failed to add creator: {error}");

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

/// Open the Edit Creator modal with existing values prefilled.
async fn edit_creator(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT name, twitch_username, youtube_id, discord_invite_code
         FROM creators
         WHERE id = ?",
    )
    .bind(creator_id)
    .fetch_optional(&ctx.data().stats_db)
    .await?;

    let Some(row) = row else {
        respond_component(ctx, interaction, "❌ That creator no longer exists.").await?;

        return Ok(());
    };

    let name: String = row.try_get("name")?;
    let twitch: Option<String> = row.try_get("twitch_username")?;
    let youtube: Option<String> = row.try_get("youtube_id")?;
    let discord: Option<String> = row.try_get("discord_invite_code")?;

    let modal = CreateQuickModal::new("Edit Creator")
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(InputTextStyle::Short, "Creator name", "")
                .value(name)
                .required(true),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "Twitch username", "")
                .value(twitch.unwrap_or_default())
                .required(false),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "YouTube channel ID", "")
                .value(youtube.unwrap_or_default())
                .required(false),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "Discord invite code", "")
                .value(discord.unwrap_or_default())
                .required(false),
        );

    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        let inputs = response.inputs;

        if inputs.len() < 4 {
            return Ok(());
        }

        let name = inputs[0].trim().to_string();

        if name.is_empty() {
            respond(
                ctx,
                &response.interaction,
                "❌ Creator name cannot be empty.",
            )
            .await?;

            return Ok(());
        }

        let result = sqlx::query(
            "UPDATE creators
             SET name = ?,
                 twitch_username = ?,
                 youtube_id = ?,
                 discord_invite_code = ?
             WHERE id = ?",
        )
        .bind(&name)
        .bind(optional_input(&inputs[1]))
        .bind(optional_input(&inputs[2]))
        .bind(optional_input(&inputs[3]))
        .bind(creator_id)
        .execute(&ctx.data().stats_db)
        .await;

        match result {
            Ok(_) => {
                respond(
                    ctx,
                    &response.interaction,
                    &format!("✅ Updated **{}** successfully!", name),
                )
                .await?;
            }
            Err(error) => {
                tracing::error!("Failed to update creator {creator_id}: {error}");

                respond(
                    ctx,
                    &response.interaction,
                    "❌ Failed to update the creator. Check the bot logs.",
                )
                .await?;
            }
        }
    }

    Ok(())
}

/// Open the settings modal with existing values prefilled.
async fn edit_settings(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT
            name,
            alert_threshold,
            check_delay_minutes,
            channel_id,
            dashboard_channel_id
        FROM creators
        WHERE id = ?",
    )
    .bind(creator_id)
    .fetch_optional(&ctx.data().stats_db)
    .await?;

    let Some(row) = row else {
        respond_component(ctx, interaction, "❌ That creator no longer exists.").await?;

        return Ok(());
    };

    let name: String = row.try_get("name")?;

    let threshold: i64 = row
        .try_get::<Option<i64>, _>("alert_threshold")?
        .unwrap_or(10);

    let delay: i64 = row
        .try_get::<Option<i64>, _>("check_delay_minutes")?
        .unwrap_or(10);

    let channel_id: Option<i64> = row.try_get("channel_id")?;

    let dashboard_channel_id: Option<i64> = row.try_get("dashboard_channel_id")?;

    let modal = CreateQuickModal::new("Creator Settings")
        .timeout(PANEL_TIMEOUT)
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Twitch milestone threshold (0 disables)",
                "",
            )
            .value(threshold.to_string())
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Growth check interval in minutes (1-1440)",
                "",
            )
            .value(delay.to_string())
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Milestone alert channel ID (0 clears)",
                "",
            )
            .value(
                channel_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            )
            .required(true),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Dashboard channel ID (0 disables)",
                "",
            )
            .value(
                dashboard_channel_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            )
            .required(true),
        );

    if let Some(response) = interaction
        .quick_modal(ctx.serenity_context(), modal)
        .await?
    {
        let inputs = response.inputs;

        if inputs.len() < 4 {
            return Ok(());
        }

        let threshold = match inputs[0].trim().parse::<i64>() {
            Ok(value) if (0..=1_000_000_000).contains(&value) => value,
            _ => {
                respond(
                    ctx,
                    &response.interaction,
                    "❌ Milestone threshold must be between 0 and 1,000,000,000.",
                )
                .await?;

                return Ok(());
            }
        };

        let delay = match inputs[1].trim().parse::<i64>() {
            Ok(value) if (1..=1440).contains(&value) => value,
            _ => {
                respond(
                    ctx,
                    &response.interaction,
                    "❌ Growth check interval must be between 1 and 1440 minutes.",
                )
                .await?;

                return Ok(());
            }
        };

        let channel_input = match inputs[2].trim().parse::<i64>() {
            Ok(value) if value >= 0 => value,
            _ => {
                respond(
                    ctx,
                    &response.interaction,
                    "❌ Channel ID must be a positive Discord channel ID, or 0 to clear it.",
                )
                .await?;

                return Ok(());
            }
        };

        let dashboard_channel_input = match inputs[3].trim().parse::<i64>() {
            Ok(value) if value >= 0 => value,
            _ => {
                respond(
            ctx,
            &response.interaction,
            "❌ Dashboard channel ID must be a positive Discord channel ID, or 0 to disable it."
        ).await?;

                return Ok(());
            }
        };

        let dashboard_channel_id = if dashboard_channel_input == 0 {
            None
        } else {
            Some(dashboard_channel_input)
        };

        let channel_id = if channel_input == 0 {
            None
        } else {
            Some(channel_input)
        };

        let result = sqlx::query(
            "UPDATE creators
            SET alert_threshold = ?,
                check_delay_minutes = ?,
                channel_id = ?,
                dashboard_channel_id = ?
            WHERE id = ?",
        )
        .bind(threshold)
        .bind(delay)
        .bind(channel_id)
        .bind(dashboard_channel_id)
        .bind(creator_id)
        .execute(&ctx.data().stats_db)
        .await;

        match result {
            Ok(_) => {
                let threshold_text = if threshold == 0 {
                    "Disabled".to_string()
                } else {
                    threshold.to_string()
                };

                let channel_text = channel_id
                    .map(|id| format!("<#{id}>"))
                    .unwrap_or_else(|| "Not configured".to_string());

                let dashboard_channel_text = dashboard_channel_id
                    .map(|id| format!("<#{id}>"))
                    .unwrap_or_else(|| "Disabled".to_string());

                respond(
                    ctx,
                    &response.interaction,
                    &format!(
                        "✅ Updated settings for **{name}**!\n\n\
                         **Milestone threshold:** {threshold_text}\n\
                         **Growth check interval:** {delay} minutes\n\
                         **Milestone alert channel:** {channel_text}\n\
                         **Dashboard channel:** {dashboard_channel_text}"
                    ),
                )
                .await?;
            }
            Err(error) => {
                tracing::error!("Failed to update settings for creator {creator_id}: {error}");

                respond(
                    ctx,
                    &response.interaction,
                    "❌ Failed to update settings. Check the bot logs.",
                )
                .await?;
            }
        }
    }

    Ok(())
}

/// Confirm and delete a creator, their metrics, and their dashboard message.
async fn delete_creator(
    ctx: Context<'_>,
    interaction: &serenity::all::ComponentInteraction,
    creator_id: i64,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT name, dashboard_channel_id, sticky_message_id
        FROM creators
        WHERE id = ?",
    )
    .bind(creator_id)
    .fetch_optional(&ctx.data().stats_db)
    .await?;

    let Some(row) = row else {
        respond_component(ctx, interaction, "❌ That creator no longer exists.").await?;

        return Ok(());
    };

    let name: String = row.try_get("name")?;
    let dashboard_channel_id: Option<i64> = row.try_get("dashboard_channel_id")?;
    let dashboard_message_id: Option<i64> = row.try_get("sticky_message_id")?;

    let components = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("creator_delete_confirm:{creator_id}"))
            .label("Confirm Delete")
            .style(ButtonStyle::Danger),
        CreateButton::new(format!("creator_delete_cancel:{creator_id}"))
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ])];

    interaction
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(format!(
                        "⚠️ **Are you absolutely sure you want to delete {name}?**\n\n\
                         This will permanently delete:\n\
                         • The creator configuration\n\
                         • All historical metrics and statistics\n\
                         • Their Discord dashboard message, if one exists\n\n\
                         **This cannot be undone.**"
                    ))
                    .components(components)
                    .ephemeral(true),
            ),
        )
        .await?;

    let collector = ComponentInteractionCollector::new(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .filter(move |interaction| {
            interaction.data.custom_id == format!("creator_delete_confirm:{creator_id}")
                || interaction.data.custom_id == format!("creator_delete_cancel:{creator_id}")
        })
        .timeout(PANEL_TIMEOUT);

    let Some(confirmation) = collector.next().await else {
        return Ok(());
    };

    if confirmation.data.custom_id == format!("creator_delete_cancel:{creator_id}") {
        confirmation
            .create_response(
                ctx.serenity_context(),
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("✅ Deletion cancelled. Nothing was changed.")
                        .ephemeral(true),
                ),
            )
            .await?;

        return Ok(());
    }

    // Acknowledge the interaction before performing database/API work.
    confirmation
        .create_response(
            ctx.serenity_context(),
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(format!("🗑️ Deleting **{name}**..."))
                    .ephemeral(true),
            ),
        )
        .await?;

    // Delete the dashboard message if one has been configured.
    let dashboard_deleted =
        if let (Some(channel), Some(message)) = (dashboard_channel_id, dashboard_message_id) {
            match (u64::try_from(channel), u64::try_from(message)) {
                (Ok(channel), Ok(message)) => {
                    match ChannelId::new(channel)
                        .delete_message(&ctx.serenity_context().http, MessageId::new(message))
                        .await
                    {
                        Ok(()) => true,
                        Err(error) => {
                            tracing::warn!(
                                "Could not delete dashboard for creator {creator_id}: {error}"
                            );
                            false
                        }
                    }
                }
                _ => {
                    tracing::warn!("Invalid dashboard IDs for creator {creator_id}");
                    false
                }
            }
        } else {
            true
        };

    // Delete metrics and creator configuration atomically.
    let result = async {
        let mut tx = ctx.data().stats_db.begin().await?;

        sqlx::query("DELETE FROM metrics WHERE creator_id = ?")
            .bind(creator_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("DELETE FROM creators WHERE id = ?")
            .bind(creator_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok::<(), sqlx::Error>(())
    }
    .await;

    match result {
        Ok(()) => {
            let dashboard_note = if dashboard_deleted {
                String::new()
            } else {
                "\n\n⚠️ I couldn't delete the Discord dashboard message. \
                 You may need to remove it manually."
                    .to_string()
            };

            ctx.send(
                poise::CreateReply::default()
                    .content(format!(
                        "✅ **{name}** and all associated historical metrics \
                         have been deleted.{dashboard_note}"
                    ))
                    .ephemeral(true),
            )
            .await?;
        }
        Err(error) => {
            tracing::error!("Failed to delete creator {creator_id}: {error}");

            ctx.send(
                poise::CreateReply::default()
                    .content(
                        "❌ Failed to delete the creator from the database. \
                         Check the bot logs.",
                    )
                    .ephemeral(true),
            )
            .await?;
        }
    }

    Ok(())
}

/// Respond to a component interaction with an ephemeral message.
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

/// Convert an optional field into an Option<String>.
fn optional_input(value: &str) -> Option<String> {
    let value = value.trim();

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Configure and manage creator statistics tracking.
#[poise::command(slash_command, subcommands("manage"))]
pub async fn creator(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}
