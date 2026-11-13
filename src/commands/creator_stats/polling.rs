use crate::commands::creator_stats::api::{get_discord_members, get_youtube_subscribers};
use crate::Data;
use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, Utc};
use poise::serenity_prelude as serenity;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

static CREATOR_LOCKS: OnceLock<Mutex<HashMap<i64, Arc<Mutex<()>>>>> = OnceLock::new();

fn creator_locks() -> &'static Mutex<HashMap<i64, Arc<Mutex<()>>>> {
    CREATOR_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn get_creator_lock(creator_id: i64) -> Arc<Mutex<()>> {
    let mut locks = creator_locks().lock().await;

    locks
        .entry(creator_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn parse_timestamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?;

    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Some(parsed.and_utc());
    }

    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(parsed.and_utc());
    }

    None
}

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
                "[CreatorStats] Failed to load {column} baseline for creator {creator_id}: {error}"
            );
            0
        }
    }
}

async fn refresh_creator(ctx: &serenity::Context, data: &Data, creator_id: i64) {
    let lock = get_creator_lock(creator_id).await;
    let _guard = lock.lock().await;

    if let Err(error) = refresh_creator_locked(ctx, data, creator_id).await {
        eprintln!("[CreatorStats] Failed to refresh creator {creator_id}: {error}");
    }
}

async fn refresh_creator_locked(
    ctx: &serenity::Context,
    data: &Data,
    creator_id: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query("SELECT * FROM creators WHERE id = ?")
        .bind(creator_id)
        .fetch_optional(&data.stats_db)
        .await?;

    let Some(row) = row else {
        return Ok(());
    };

    let name: String = row
        .try_get::<Option<String>, _>("name")?
        .unwrap_or_else(|| "Unknown Creator".to_string());

    let twitch_username: Option<String> = row.try_get("twitch_username")?;
    let youtube_id: Option<String> = row.try_get("youtube_id")?;
    let discord_invite: Option<String> = row.try_get("discord_invite_code")?;
    let channel_id: Option<i64> = row.try_get("channel_id")?;

    let alert_threshold: i64 = row
        .try_get::<Option<i64>, _>("alert_threshold")?
        .unwrap_or(10);

    let check_delay: i64 = row
        .try_get::<Option<i64>, _>("check_delay_minutes")?
        .unwrap_or(10)
        .max(1);

    let last_growth_check: Option<String> = row.try_get("last_growth_check")?;
    let last_discord_check: Option<String> = row.try_get("last_discord_check")?;

    let last_alerted_twitch: i64 = row
        .try_get::<Option<i64>, _>("last_alerted_twitch")?
        .unwrap_or(0);

    let cached_twitch_id: Option<String> = row.try_get("twitch_user_id")?;

    let now = Utc::now();
    let now_string = now.to_rfc3339();

    let last_growth = parse_timestamp(last_growth_check.as_deref());

    let full_growth_check = last_growth
        .map(|last| now - last >= ChronoDuration::minutes(check_delay))
        .unwrap_or(true);

    let last_discord = parse_timestamp(last_discord_check.as_deref());

    let discord_check_due = last_discord
        .map(|last| now - last >= ChronoDuration::hours(1))
        .unwrap_or(true);

    // Load the latest known good growth values.
    let mut twitch_followers =
        latest_nonzero_metric(&data.stats_db, creator_id, "twitch_followers").await;

    let mut yt_subs = latest_nonzero_metric(&data.stats_db, creator_id, "yt_subs").await;

    let mut discord_members =
        latest_nonzero_metric(&data.stats_db, creator_id, "discord_members").await;

    // Keep the old values so we can tell whether the graph needs rebuilding.
    let previous_twitch_followers = twitch_followers;
    let previous_yt_subs = yt_subs;
    let previous_discord_members = discord_members;

    // Twitch live status is checked every refresh, independently of growth stats.
    let twitch_stats = if let Some(username) = twitch_username.as_deref() {
        if !username.trim().is_empty() {
            Some(
                data.twitch
                    .get_stats(username, cached_twitch_id.as_deref())
                    .await?,
            )
        } else {
            None
        }
    } else {
        None
    };

    let is_live = twitch_stats
        .as_ref()
        .map(|stats| stats.is_live)
        .unwrap_or(false);

    let viewers = twitch_stats
        .as_ref()
        .map(|stats| stats.viewers)
        .unwrap_or(0);

    let stream_title = twitch_stats.as_ref().and_then(|stats| stats.title.clone());

    let thumbnail_url = twitch_stats
        .as_ref()
        .and_then(|stats| stats.thumbnail_url.clone());

    let game_name = twitch_stats
        .as_ref()
        .and_then(|stats| stats.game_name.clone());

    // Cache the resolved Twitch user ID.
    if let Some(stats) = twitch_stats.as_ref() {
        if cached_twitch_id.as_deref() != Some(stats.user_id.as_str()) {
            sqlx::query("UPDATE creators SET twitch_user_id = ? WHERE id = ?")
                .bind(&stats.user_id)
                .bind(creator_id)
                .execute(&data.stats_db)
                .await?;
        }
    }

    let mut discord_check_succeeded = false;

    if full_growth_check {
        // Twitch follower count may be unavailable due to Twitch authorization.
        // Preserve the previous value when it cannot be retrieved.
        if let Some(followers) = twitch_stats.as_ref().and_then(|stats| stats.followers) {
            if followers > 0 {
                twitch_followers = followers as i64;
            }
        }

        if let Some(channel) = youtube_id.as_deref() {
            if !channel.trim().is_empty() {
                match get_youtube_subscribers(&data.http, channel).await {
                    Ok(Some(subscribers)) if subscribers > 0 => {
                        yt_subs = subscribers as i64;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("[CreatorStats] YouTube request failed for {name}: {error}");
                    }
                }
            }
        }

        if discord_check_due {
            if let Some(invite) = discord_invite.as_deref() {
                if !invite.trim().is_empty() {
                    match get_discord_members(&data.http, invite).await {
                        Ok(Some(members)) if members > 0 => {
                            discord_members = members as i64;
                            discord_check_succeeded = true;
                        }
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!(
                                "[CreatorStats] Discord invite request failed for {name}: {error}"
                            );
                        }
                    }
                }
            }
        }
    }

    // Save a snapshot on every successful refresh. The chart itself is only
    // regenerated when one of the underlying growth values actually changes.
    sqlx::query(
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
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&now_string)
    .bind(creator_id)
    .bind(yt_subs)
    .bind(twitch_followers)
    .bind(discord_members)
    .bind(if is_live { 1_i64 } else { 0_i64 })
    .bind(viewers as i64)
    .bind(&stream_title)
    .bind(&thumbnail_url)
    .bind(&game_name)
    .execute(&data.stats_db)
    .await?;

    // Update creator refresh timestamps and current live state.
    sqlx::query(
        r#"
        UPDATE creators
        SET
            is_currently_live = ?,
            last_check_time = ?,
            last_growth_check = CASE
                WHEN ? THEN ?
                ELSE last_growth_check
            END,
            last_discord_check = CASE
                WHEN ? THEN ?
                ELSE last_discord_check
            END
        WHERE id = ?
        "#,
    )
    .bind(if is_live { 1_i64 } else { 0_i64 })
    .bind(&now_string)
    .bind(full_growth_check)
    .bind(&now_string)
    .bind(discord_check_succeeded)
    .bind(&now_string)
    .bind(creator_id)
    .execute(&data.stats_db)
    .await?;

    // Twitch follower milestone detection.
    if alert_threshold > 0 && twitch_followers > 0 {
        let milestone = (twitch_followers / alert_threshold) * alert_threshold;

        if last_alerted_twitch <= 0 {
            // Establish the initial baseline without alerting for old milestones.
            sqlx::query("UPDATE creators SET last_alerted_twitch = ? WHERE id = ?")
                .bind(milestone)
                .bind(creator_id)
                .execute(&data.stats_db)
                .await?;
        } else if milestone > last_alerted_twitch {
            if let Some(channel_id) = channel_id {
                if let Ok(channel_id) = u64::try_from(channel_id) {
                    let message = format!(
                        "🎉 **{name}** just hit a new milestone: **{milestone}** Twitch Followers! 🚀"
                    );

                    match serenity::ChannelId::new(channel_id)
                        .send_message(&ctx.http, serenity::CreateMessage::new().content(message))
                        .await
                    {
                        Ok(_) => {
                            sqlx::query("UPDATE creators SET last_alerted_twitch = ? WHERE id = ?")
                                .bind(milestone)
                                .bind(creator_id)
                                .execute(&data.stats_db)
                                .await?;
                        }
                        Err(error) => {
                            eprintln!(
                                "[CreatorStats] Failed to send milestone alert for {name}: {error}"
                            );
                        }
                    }
                }
            }
        }
    }

    let growth_metrics_changed = twitch_followers != previous_twitch_followers
        || yt_subs != previous_yt_subs
        || discord_members != previous_discord_members;

    if let Err(error) = crate::commands::creator_stats::dashboard::update_dashboard_with_chart(
        ctx,
        data,
        creator_id,
        growth_metrics_changed,
    )
    .await
    {
        eprintln!("[CreatorStats] Dashboard update failed for creator {creator_id}: {error}");
    }

    println!(
        "[CreatorStats] Refreshed {name} | Twitch: {twitch_followers} | YouTube: {yt_subs} | Discord: {discord_members} | Live: {is_live}"
    );

    Ok(())
}

pub async fn refresh_all(ctx: &serenity::Context, data: &Data) {
    let rows = match sqlx::query("SELECT id FROM creators ORDER BY id")
        .fetch_all(&data.stats_db)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("[CreatorStats] Failed to load creators: {error}");
            return;
        }
    };

    for row in rows {
        let creator_id: i64 = match row.try_get("id") {
            Ok(id) => id,
            Err(error) => {
                eprintln!("[CreatorStats] Invalid creator ID: {error}");
                continue;
            }
        };

        refresh_creator(ctx, data, creator_id).await;
    }
}

pub fn start_background(ctx: serenity::Context, data: Data) {
    tokio::spawn(async move {
        println!("[CreatorStats] Background polling started.");

        loop {
            refresh_all(&ctx, &data).await;
            sleep(Duration::from_secs(60)).await;
        }
    });
}
