use sqlx::{Row, SqlitePool};

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<(), sqlx::Error> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?;

    let exists = rows.iter().any(|row| {
        let name: String = row.get("name");
        name == column
    });

    if !exists {
        sqlx::query(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))
        .execute(pool)
        .await?;
    }

    Ok(())
}

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS creators (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT,
            youtube_id TEXT,
            twitch_username TEXT,
            discord_invite_code TEXT,
            channel_id INTEGER,
            alert_threshold INTEGER DEFAULT 10,
            check_delay_minutes INTEGER DEFAULT 10,
            last_check_time DATETIME,
            last_discord_check DATETIME,
            sticky_message_id INTEGER,
            last_alerted_twitch INTEGER DEFAULT 0,
            last_alerted_yt INTEGER DEFAULT 0,
            live_alert_channel_id INTEGER,
            live_alert_role_id INTEGER,
            is_currently_live BOOLEAN DEFAULT 0,
            last_growth_check DATETIME,
            twitch_user_id TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS metrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp DATETIME,
            creator_id INTEGER,
            yt_subs INTEGER,
            twitch_followers INTEGER,
            discord_members INTEGER,
            is_live BOOLEAN,
            viewers INTEGER,
            stream_title TEXT,
            thumbnail_url TEXT,
            game_name TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Migrations for databases created by older versions of the Python bot.
    ensure_column(pool, "creators", "live_alert_channel_id", "INTEGER").await?;
    ensure_column(pool, "creators", "live_alert_role_id", "INTEGER").await?;
    ensure_column(pool, "creators", "is_currently_live", "BOOLEAN DEFAULT 0").await?;
    ensure_column(pool, "creators", "last_growth_check", "DATETIME").await?;
    ensure_column(pool, "creators", "twitch_user_id", "TEXT").await?;
    ensure_column(pool, "creators", "dashboard_channel_id", "INTEGER").await?;

    ensure_column(pool, "metrics", "stream_title", "TEXT").await?;
    ensure_column(pool, "metrics", "thumbnail_url", "TEXT").await?;
    ensure_column(pool, "metrics", "game_name", "TEXT").await?;

    // Useful indexes for polling, dashboard history, and GoLive lookups.
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_metrics_creator_timestamp \
         ON metrics (creator_id, timestamp)",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_creators_twitch_username \
         ON creators (twitch_username)",
    )
    .execute(pool)
    .await?;

    println!("📊 CreatorStats database initialized.");

    Ok(())
}
