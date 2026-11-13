use crate::Data;
use chrono::Utc;
use poise::serenity_prelude as serenity;
use sqlx::{Row, SqlitePool};
use tokio::time::{sleep, Duration};

async fn latest_nonzero_metric(pool: &SqlitePool, creator_id: i64, column: &str) -> i64 {
    // Column names are selected internally from fixed values below.
    let query = format!(
        "SELECT {column} FROM metrics \
         WHERE creator_id = ? AND {column} > 0 \
         ORDER BY timestamp DESC, id DESC LIMIT 1"
    );

    match sqlx::query(&query)
        .bind(creator_id)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(row)) => row.try_get::<i64, _>(column).unwrap_or(0),
        Ok(None) => 0,
        Err(error) => {
            eprintln!(
                "[GoLive] Failed to load {column} baseline for creator {creator_id}: {error}"
            );
            0
        }
    }
}

pub(super) fn format_alert_message(
    template: Option<&str>,
    name: &str,
    username: &str,
    title: Option<&str>,
    game: Option<&str>,
    role_id: Option<i64>,
) -> String {
    let ping = role_id
        .and_then(|id| u64::try_from(id).ok())
        .map(|id| format!("<@&{id}>"))
        .unwrap_or_default();

    let title = title.unwrap_or("Untitled stream");
    let game = game.unwrap_or("Unknown category");
    let url = format!("https://twitch.tv/{username}");

    // Default format matches the original Python bot.
    let ping_prefix = if ping.is_empty() {
        String::new()
    } else {
        format!("{ping} ")
    };

    let default_message = format!(
        "{ping_prefix}🔴 **{name}** is now LIVE on Twitch!\n\
         **Playing:** {game}\n\
         **Title:** {title}\n\
         {url}"
    );

    let Some(template) = template.filter(|message| !message.trim().is_empty()) else {
        return default_message;
    };

    // Preserve custom-message placeholder behavior.
    template
        .replace("${ping}", &ping)
        .replace("${creator}", name)
        .replace("${name}", name)
        .replace("${twitch_username}", username)
        .replace("${username}", username)
        .replace("${title}", title)
        .replace("${category}", game)
        .replace("${game}", game)
        .replace("${url}", &url)
        .replace("${link}", &url)
}

async fn record_transition_metrics(
    ctx: &serenity::Context,
    data: &Data,
    username: &str,
    stats: &crate::commands::creator_stats::api::TwitchStats,
) {
    let row = match sqlx::query(
        "SELECT id FROM creators WHERE LOWER(twitch_username) = LOWER(?) LIMIT 1",
    )
    .bind(username)
    .fetch_optional(&data.stats_db)
    .await
    {
        Ok(row) => row,
        Err(error) => {
            eprintln!("[GoLive] Failed to find linked CreatorStats entry: {error}");
            return;
        }
    };

    let Some(row) = row else {
        return;
    };

    let creator_id: i64 = match row.try_get("id") {
        Ok(id) => id,
        Err(error) => {
            eprintln!("[GoLive] Invalid linked CreatorStats ID: {error}");
            return;
        }
    };

    let twitch_followers =
        latest_nonzero_metric(&data.stats_db, creator_id, "twitch_followers").await;
    let yt_subs = latest_nonzero_metric(&data.stats_db, creator_id, "yt_subs").await;
    let discord_members =
        latest_nonzero_metric(&data.stats_db, creator_id, "discord_members").await;

    let now = Utc::now().to_rfc3339();

    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO metrics (
            timestamp,
            creator_id,
            yt_subs,
            twitch_followers,
            discord_members,
            is_live,
            viewers,
            stream_title,
            thumbnail_url,
            game_name
        )
        VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?, ?)
        "#,
    )
    .bind(&now)
    .bind(creator_id)
    .bind(yt_subs)
    .bind(twitch_followers)
    .bind(discord_members)
    .bind(stats.viewers as i64)
    .bind(&stats.title)
    .bind(&stats.thumbnail_url)
    .bind(&stats.game_name)
    .execute(&data.stats_db)
    .await
    {
        eprintln!("[GoLive] Failed to record transition metrics: {error}");
        return;
    }

    println!("[GoLive] Recorded live transition metrics for {username}.");
    if let Err(error) =
        crate::commands::creator_stats::dashboard::update_dashboard(ctx, data, creator_id).await
    {
        eprintln!("[GoLive] Dashboard update failed for creator {creator_id}: {error}");
    }

    // Refresh the linked CreatorStats dashboard using the new snapshot.
    // This does not trigger another Twitch API request.
    // The caller supplies the Serenity context separately.
}

async fn check_creator(ctx: &serenity::Context, data: &Data, row: sqlx::sqlite::SqliteRow) {
    let id: i64 = match row.try_get("id") {
        Ok(value) => value,
        Err(error) => {
            eprintln!("[GoLive] Invalid creator ID: {error}");
            return;
        }
    };

    // GoLive configurations are strictly guild-scoped.
    let guild_id: i64 = match row.try_get("guild_id") {
        Ok(value) => value,
        Err(error) => {
            eprintln!("[GoLive] Invalid guild ID for creator {id}: {error}");
            return;
        }
    };

    let name: String = row
        .try_get::<Option<String>, _>("name")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Unknown Creator".to_string());

    let Some(username) = row
        .try_get::<Option<String>, _>("twitch_username")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
    else {
        return;
    };

    let cached_user_id: Option<String> = row.try_get("twitch_user_id").unwrap_or(None);

    let alert_channel_id: Option<i64> = row.try_get("live_alert_channel_id").unwrap_or(None);

    let alert_role_id: Option<i64> = row.try_get("live_alert_role_id").unwrap_or(None);

    let custom_message: Option<String> = row.try_get("custom_message").unwrap_or(None);

    let was_live: bool = row
        .try_get::<Option<i64>, _>("is_currently_live")
        .unwrap_or(Some(0))
        .unwrap_or(0)
        != 0;

    // An API error is not an offline result. Leave the stored state untouched.
    let stats = match data
        .twitch
        .get_stats(&username, cached_user_id.as_deref())
        .await
    {
        Ok(stats) => stats,
        Err(error) => {
            eprintln!("[GoLive] Twitch check failed for {username}: {error}");
            return;
        }
    };

    // Cache the resolved Twitch ID, scoped to this guild and configuration.
    if cached_user_id.as_deref() != Some(stats.user_id.as_str()) {
        if let Err(error) = sqlx::query(
            "UPDATE go_live_creators SET twitch_user_id = ? WHERE id = ? AND guild_id = ?",
        )
        .bind(&stats.user_id)
        .bind(id)
        .bind(guild_id)
        .execute(&data.stats_db)
        .await
        {
            eprintln!("[GoLive] Failed to cache Twitch ID for {username}: {error}");
        }
    }

    // No transition means no work.
    if stats.is_live == was_live {
        return;
    }

    // Persist the transition before sending notifications.
    if let Err(error) = sqlx::query(
        "UPDATE go_live_creators SET is_currently_live = ? WHERE id = ? AND guild_id = ?",
    )
    .bind(if stats.is_live { 1_i64 } else { 0_i64 })
    .bind(id)
    .bind(guild_id)
    .execute(&data.stats_db)
    .await
    {
        eprintln!("[GoLive] Failed to update live state for {username}: {error}");
        return;
    }

    if stats.is_live {
        println!("[GoLive] 🔴 {name} ({username}) went live.");

        // Record the transition in CreatorStats, if this Twitch account is
        // also configured there.
        record_transition_metrics(ctx, data, &username, &stats).await;

        if let Some(channel_id) = alert_channel_id {
            if let Ok(channel_id) = u64::try_from(channel_id) {
                let message = format_alert_message(
                    custom_message.as_deref(),
                    &name,
                    &username,
                    stats.title.as_deref(),
                    stats.game_name.as_deref(),
                    alert_role_id,
                );

                match serenity::ChannelId::new(channel_id)
                    .send_message(&ctx.http, serenity::CreateMessage::new().content(message))
                    .await
                {
                    Ok(_) => {
                        println!("[GoLive] Sent live alert for {username}.");
                    }
                    Err(error) => {
                        eprintln!("[GoLive] Failed to send live alert for {username}: {error}");
                    }
                }
            }
        }
    } else {
        println!("[GoLive] ⚫ {name} ({username}) went offline.");
    }
}

pub async fn refresh_all(ctx: &serenity::Context, data: &Data) {
    let rows = match sqlx::query(
        r#"
        SELECT
            id,
            guild_id,
            name,
            twitch_username,
            twitch_user_id,
            live_alert_channel_id,
            live_alert_role_id,
            is_currently_live,
            custom_message
        FROM go_live_creators
        WHERE guild_id IS NOT NULL
          AND is_enabled = 1
          AND TRIM(COALESCE(twitch_username, '')) <> ''
        ORDER BY guild_id, id
        "#,
    )
    .fetch_all(&data.stats_db)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("[GoLive] Failed to load configured creators: {error}");
            return;
        }
    };

    for row in rows {
        check_creator(ctx, data, row).await;
    }
}

pub fn start_background(ctx: serenity::Context, data: Data) {
    tokio::spawn(async move {
        println!("[GoLive] Background polling started.");

        loop {
            refresh_all(&ctx, &data).await;
            sleep(Duration::from_secs(60)).await;
        }
    });
}
