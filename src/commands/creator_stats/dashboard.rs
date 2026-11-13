use crate::Data;
use poise::serenity_prelude as serenity;
use serenity::{
    ChannelId, CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMessage,
    MessageId,
};
use sqlx::Row;

type DashboardError = Box<dyn std::error::Error + Send + Sync>;

pub async fn update_dashboard(
    ctx: &serenity::Context,
    data: &Data,
    creator_id: i64,
) -> Result<(), DashboardError> {
    update_dashboard_with_chart(ctx, data, creator_id, false).await
}

pub async fn update_dashboard_with_chart(
    ctx: &serenity::Context,
    data: &Data,
    creator_id: i64,
    refresh_chart: bool,
) -> Result<(), DashboardError> {
    let creator = sqlx::query(
        r#"
        SELECT
            name,
            twitch_username,
            youtube_id,
            discord_invite_code,
            dashboard_channel_id,
            sticky_message_id
        FROM creators
        WHERE id = ?
        "#,
    )
    .bind(creator_id)
    .fetch_optional(&data.stats_db)
    .await?;

    let Some(creator) = creator else {
        return Ok(());
    };

    let name: String = creator
        .try_get::<Option<String>, _>("name")?
        .unwrap_or_else(|| "Unknown Creator".to_string());

    let twitch_username: Option<String> = creator.try_get("twitch_username")?;
    let youtube_id: Option<String> = creator.try_get("youtube_id")?;
    let discord_invite: Option<String> = creator.try_get("discord_invite_code")?;
    let dashboard_channel_id: Option<i64> = creator.try_get("dashboard_channel_id")?;
    let sticky_message_id: Option<i64> = creator.try_get("sticky_message_id")?;

    let Some(channel_id) = dashboard_channel_id else {
        return Ok(());
    };

    let channel_id = u64::try_from(channel_id)?;

    let metrics = sqlx::query(
        r#"
        SELECT
            yt_subs,
            twitch_followers,
            discord_members,
            is_live,
            viewers,
            stream_title,
            game_name
        FROM metrics
        WHERE creator_id = ?
        ORDER BY timestamp DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(creator_id)
    .fetch_optional(&data.stats_db)
    .await?;

    let yt_subs = metrics
        .as_ref()
        .and_then(|row| row.try_get::<Option<i64>, _>("yt_subs").ok().flatten())
        .unwrap_or(0);

    let twitch_followers = metrics
        .as_ref()
        .and_then(|row| {
            row.try_get::<Option<i64>, _>("twitch_followers")
                .ok()
                .flatten()
        })
        .unwrap_or(0);

    let discord_members = metrics
        .as_ref()
        .and_then(|row| {
            row.try_get::<Option<i64>, _>("discord_members")
                .ok()
                .flatten()
        })
        .unwrap_or(0);

    let is_live = metrics
        .as_ref()
        .and_then(|row| row.try_get::<Option<i64>, _>("is_live").ok().flatten())
        .unwrap_or(0)
        != 0;

    let viewers = metrics
        .as_ref()
        .and_then(|row| row.try_get::<Option<i64>, _>("viewers").ok().flatten())
        .unwrap_or(0);

    let stream_title: Option<String> = metrics
        .as_ref()
        .and_then(|row| row.try_get("stream_title").ok().flatten());

    let game_name: Option<String> = metrics
        .as_ref()
        .and_then(|row| row.try_get("game_name").ok().flatten());

    let mut description = if is_live {
        format!("🔴 **LIVE NOW** · {viewers} viewers")
    } else {
        "⚫ Currently offline".to_string()
    };

    if is_live {
        if let Some(title) = stream_title.as_deref().filter(|s| !s.is_empty()) {
            description.push_str(&format!("\n\n**Stream:** {title}"));
        }

        if let Some(game) = game_name.as_deref().filter(|s| !s.is_empty()) {
            description.push_str(&format!("\n**Playing:** {game}"));
        }
    }

    let mut embed = CreateEmbed::new()
        .title(format!("📊 {name} — Creator Stats"))
        .description(description)
        .color(if is_live { 0xE74C3C } else { 0x9146FF })
        .field("Twitch Followers", format!("{twitch_followers}"), true)
        .field("YouTube Subscribers", format!("{yt_subs}"), true)
        .field("Discord Members", format!("{discord_members}"), true)
        .footer(CreateEmbedFooter::new(
            "CyberPup Bot · Updates automatically",
        ));

    if let Some(username) = twitch_username.as_deref().filter(|s| !s.is_empty()) {
        embed = embed.url(format!("https://twitch.tv/{username}"));
    }

    if let Some(invite) = discord_invite.as_deref().filter(|s| !s.is_empty()) {
        embed = embed.field(
            "Discord",
            format!("[Join the community](https://discord.gg/{invite})"),
            false,
        );
    }

    if let Some(channel) = youtube_id.as_deref().filter(|s| !s.is_empty()) {
        embed = embed.field(
            "YouTube",
            format!("[Visit channel](https://youtube.com/channel/{channel})"),
            false,
        );
    }

    let chart_path = format!("/tmp/cyberpup_creator_{creator_id}_growth.png");
    let channel = ChannelId::new(channel_id);

    let existing_message_id = sticky_message_id.and_then(|id| u64::try_from(id).ok());

    // A missing dashboard needs a fresh chart regardless of the growth schedule.
    let should_generate_chart = refresh_chart || existing_message_id.is_none();

    let chart_available = if should_generate_chart {
        match crate::commands::creator_stats::charts::generate_growth_chart(
            &data.stats_db,
            creator_id,
            &chart_path,
        )
        .await
        {
            Ok(()) => true,
            Err(error) => {
                eprintln!("[CreatorStats] Could not generate growth chart for {name}: {error}");
                false
            }
        }
    } else {
        true
    };

    let mut dashboard_embed = embed.clone();

    if chart_available {
        dashboard_embed = dashboard_embed.image("attachment://growth.png");
    }

    // Edit the existing dashboard. When the chart hasn't changed, leave its
    // existing attachment alone instead of uploading a new copy every minute.
    if let Some(message_id) = existing_message_id {
        let mut edit = EditMessage::new().embed(dashboard_embed);

        if should_generate_chart {
            if chart_available {
                let chart_bytes = tokio::fs::read(&chart_path).await?;
                edit = edit.new_attachment(CreateAttachment::bytes(chart_bytes, "growth.png"));
            } else {
                edit = edit.remove_all_attachments();
            }
        }

        match channel
            .edit_message(&ctx.http, MessageId::new(message_id), edit)
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) => {
                eprintln!(
                    "[CreatorStats] Could not edit dashboard for {name}; \
                     creating a replacement: {error}"
                );
            }
        }
    }

    // If editing failed, a replacement message needs its own chart attachment.
    let replacement_chart_available = if should_generate_chart {
        chart_available
    } else if tokio::fs::metadata(&chart_path).await.is_ok() {
        // Reuse the previously generated graph from disk.
        true
    } else {
        // The cached graph is unavailable, so generate a replacement.
        match crate::commands::creator_stats::charts::generate_growth_chart(
            &data.stats_db,
            creator_id,
            &chart_path,
        )
        .await
        {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "[CreatorStats] Could not generate replacement chart for {name}: {error}"
                );
                false
            }
        }
    };

    let mut replacement_embed = embed;

    if replacement_chart_available {
        replacement_embed = replacement_embed.image("attachment://growth.png");
    }

    let mut message_builder = CreateMessage::new().embed(replacement_embed);

    if replacement_chart_available {
        let chart_bytes = tokio::fs::read(&chart_path).await?;
        message_builder =
            message_builder.add_file(CreateAttachment::bytes(chart_bytes, "growth.png"));
    }

    let message = channel.send_message(&ctx.http, message_builder).await?;

    sqlx::query("UPDATE creators SET sticky_message_id = ? WHERE id = ?")
        .bind(i64::try_from(message.id.get())?)
        .bind(creator_id)
        .execute(&data.stats_db)
        .await?;

    println!("[CreatorStats] Created dashboard for {name}.");

    Ok(())
}
static DASHBOARD_STICKY_TASKS: std::sync::OnceLock<
    tokio::sync::Mutex<std::collections::HashMap<u64, tokio::task::JoinHandle<()>>>,
> = std::sync::OnceLock::new();

pub async fn handle_message(ctx: &serenity::Context, data: &Data, channel_id: ChannelId) {
    let tasks = DASHBOARD_STICKY_TASKS
        .get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));

    let mut tasks = tasks.lock().await;
    let channel_key = channel_id.get();

    // Reset the pending task whenever another message arrives.
    if let Some(previous) = tasks.remove(&channel_key) {
        previous.abort();
    }

    let ctx = ctx.clone();
    let data = data.clone();

    let task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let dashboards = match sqlx::query(
            "SELECT id, sticky_message_id FROM creators WHERE dashboard_channel_id = ?",
        )
        .bind(channel_key as i64)
        .fetch_all(&data.stats_db)
        .await
        {
            Ok(rows) => rows,
            Err(error) => {
                eprintln!("[CreatorStats] Failed to find dashboards: {error}");
                return;
            }
        };

        for dashboard in dashboards {
            let creator_id: i64 = match dashboard.try_get("id") {
                Ok(id) => id,
                Err(error) => {
                    eprintln!("[CreatorStats] Invalid creator ID: {error}");
                    continue;
                }
            };

            let message_id: Option<i64> = match dashboard.try_get("sticky_message_id") {
                Ok(id) => id,
                Err(error) => {
                    eprintln!("[CreatorStats] Invalid dashboard message ID: {error}");
                    continue;
                }
            };

            // Remove the old dashboard message so the replacement appears
            // at the bottom of the channel.
            if let Some(message_id) = message_id {
                if let Ok(message_id) = u64::try_from(message_id) {
                    if let Err(error) = channel_id
                        .delete_message(&ctx.http, MessageId::new(message_id))
                        .await
                    {
                        eprintln!("[CreatorStats] Could not remove old dashboard message: {error}");
                    }
                }
            }

            // Reuse the existing chart rather than regenerating it.
            if let Err(error) = update_dashboard_with_chart(&ctx, &data, creator_id, false).await {
                eprintln!("[CreatorStats] Failed to reposition dashboard {creator_id}: {error}");
            }
        }
    });

    tasks.insert(channel_key, task);
}
