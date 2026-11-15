use crate::Data;
use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, Utc};
use poise::serenity_prelude as serenity;
use serenity::{
    ChannelId, CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage, EditMessage,
    MessageId,
};
use sqlx::Row;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};
use tokio::sync::{Mutex, OwnedMutexGuard};

type DashboardError = Box<dyn std::error::Error + Send + Sync>;

fn parse_timestamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?;

    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Some(parsed.and_utc());
    }

    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
        .ok()
        .map(|parsed| parsed.and_utc())
}

fn dashboard_marker(creator_id: i64) -> String {
    let mut marker = String::from("\u{2063}");
    let symbols = ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}'];
    for byte in creator_id.to_be_bytes() {
        for shift in [6, 4, 2, 0] {
            marker.push(symbols[((byte >> shift) & 0b11) as usize]);
        }
    }
    marker.push('\u{2063}');
    marker
}

static DASHBOARD_LOCKS: OnceLock<Mutex<HashMap<i64, Arc<Mutex<()>>>>> = OnceLock::new();

async fn lock_dashboard(creator_id: i64) -> OwnedMutexGuard<()> {
    let locks = DASHBOARD_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let lock = {
        let mut locks = locks.lock().await;
        locks
            .entry(creator_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

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
    let _guard = lock_dashboard(creator_id).await;
    update_dashboard_locked(ctx, data, creator_id, refresh_chart).await
}

async fn update_dashboard_locked(
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
            sticky_message_id,
            check_delay_minutes,
            last_check_time,
            last_growth_check
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
    let check_delay_minutes = creator
        .try_get::<Option<i64>, _>("check_delay_minutes")?
        .unwrap_or(10)
        .clamp(1, 1440);
    let discord_check_interval = check_delay_minutes.max(60);
    let last_check_time: Option<String> = creator.try_get("last_check_time")?;
    let last_growth_check: Option<String> = creator.try_get("last_growth_check")?;

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

    let now = Utc::now();
    let last_updated = parse_timestamp(last_check_time.as_deref())
        .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "pending first poll".to_string());
    let next_growth_update = parse_timestamp(last_growth_check.as_deref())
        .map(|timestamp| timestamp + ChronoDuration::minutes(check_delay_minutes))
        .filter(|timestamp| *timestamp > now)
        .unwrap_or_else(|| now + ChronoDuration::minutes(1));

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

    description.push_str(&format!(
        "\n\n⏭️ **Next growth update:** <t:{}:R>",
        next_growth_update.timestamp()
    ));

    let mut embed = CreateEmbed::new()
        .title(format!("📊 {name} — Creator Stats"))
        .description(description)
        .color(if is_live { 0xE74C3C } else { 0x9146FF })
        .field("Twitch Followers", format!("{twitch_followers}"), true)
        .field("YouTube Subscribers", format!("{yt_subs}"), true)
        .field("Discord Members", format!("{discord_members}"), true)
        .footer(CreateEmbedFooter::new(format!(
            "CyberPup Bot · Last updated at {last_updated} · Updates automatically: growth stats every {check_delay_minutes} min, live status every minute, Discord members every {discord_check_interval} min{}",
            dashboard_marker(creator_id)
        )));

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

    let mut existing_message_id = sticky_message_id.and_then(|id| u64::try_from(id).ok());

    // Recover a dashboard if Discord accepted a previous send but saving its
    // message ID to SQLite failed. The stable footer marker identifies only
    // this creator's dashboard.
    if existing_message_id.is_none() {
        let recent_messages = channel
            .messages(&ctx.http, serenity::GetMessages::new().limit(100))
            .await?;
        let bot_id = ctx.cache.current_user().id;
        let marker = dashboard_marker(creator_id);
        let legacy_marker = format!("CPD:{creator_id}");
        let mut matches = recent_messages.into_iter().filter(|message| {
            message.author.id == bot_id
                && message.embeds.iter().any(|embed| {
                    embed.footer.as_ref().is_some_and(|footer| {
                        footer.text.contains(&marker) || footer.text.contains(&legacy_marker)
                    })
                })
        });

        if let Some(message) = matches.next() {
            existing_message_id = Some(message.id.get());
            for duplicate in matches {
                if let Err(error) = channel.delete_message(&ctx.http, duplicate.id).await {
                    eprintln!(
                        "[CreatorStats] Could not remove duplicate dashboard {} for creator {creator_id}: {error}",
                        duplicate.id
                    );
                }
            }
            sqlx::query("UPDATE creators SET sticky_message_id = ? WHERE id = ?")
                .bind(i64::try_from(message.id.get())?)
                .bind(creator_id)
                .execute(&data.stats_db)
                .await?;
        }
    }

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
            Err(error)
                if matches!(
                    &error,
                    serenity::Error::Http(http_error)
                        if http_error.status_code() == Some(serenity::http::StatusCode::NOT_FOUND)
                ) =>
            {
                eprintln!("[CreatorStats] Dashboard for {name} no longer exists; replacing it.");
                sqlx::query("UPDATE creators SET sticky_message_id = NULL WHERE id = ?")
                    .bind(creator_id)
                    .execute(&data.stats_db)
                    .await?;
            }
            Err(error) => return Err(error.into()),
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

async fn reposition_dashboard(
    ctx: &serenity::Context,
    data: &Data,
    channel_id: ChannelId,
    creator_id: i64,
) -> Result<(), DashboardError> {
    let _guard = lock_dashboard(creator_id).await;

    let sticky_message_id: Option<i64> = sqlx::query_scalar(
        "SELECT sticky_message_id FROM creators WHERE id = ? AND dashboard_channel_id = ?",
    )
    .bind(creator_id)
    .bind(i64::try_from(channel_id.get())?)
    .fetch_optional(&data.stats_db)
    .await?
    .flatten();

    if let Some(message_id) = sticky_message_id.and_then(|id| u64::try_from(id).ok()) {
        match channel_id
            .delete_message(&ctx.http, MessageId::new(message_id))
            .await
        {
            Ok(()) => {}
            Err(error)
                if matches!(
                    &error,
                    serenity::Error::Http(http_error)
                        if http_error.status_code() == Some(serenity::http::StatusCode::NOT_FOUND)
                ) => {}
            Err(error) => return Err(error.into()),
        }

        sqlx::query("UPDATE creators SET sticky_message_id = NULL WHERE id = ?")
            .bind(creator_id)
            .execute(&data.stats_db)
            .await?;
    }

    update_dashboard_locked(ctx, data, creator_id, false).await
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

        let dashboards = match sqlx::query("SELECT id FROM creators WHERE dashboard_channel_id = ?")
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

            if let Err(error) = reposition_dashboard(&ctx, &data, channel_id, creator_id).await {
                eprintln!("[CreatorStats] Failed to reposition dashboard {creator_id}: {error}");
            }
        }
    });

    tasks.insert(channel_key, task);
}
