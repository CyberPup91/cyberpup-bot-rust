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
        CREATE TABLE IF NOT EXISTS go_live_creators (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            guild_id INTEGER,
            name TEXT,
            twitch_username TEXT,
            twitch_user_id TEXT,
            live_alert_channel_id INTEGER,
            live_alert_role_id INTEGER,
            is_currently_live BOOLEAN DEFAULT 0,
            is_enabled BOOLEAN DEFAULT 1,
            custom_message TEXT DEFAULT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Migrations for older Python database versions.
    ensure_column(pool, "go_live_creators", "guild_id", "INTEGER").await?;
    ensure_column(pool, "go_live_creators", "twitch_user_id", "TEXT").await?;
    ensure_column(pool, "go_live_creators", "live_alert_channel_id", "INTEGER").await?;
    ensure_column(pool, "go_live_creators", "live_alert_role_id", "INTEGER").await?;
    ensure_column(
        pool,
        "go_live_creators",
        "is_currently_live",
        "BOOLEAN DEFAULT 0",
    )
    .await?;
    ensure_column(pool, "go_live_creators", "is_enabled", "BOOLEAN DEFAULT 1").await?;
    ensure_column(
        pool,
        "go_live_creators",
        "custom_message",
        "TEXT DEFAULT NULL",
    )
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_go_live_guild \
         ON go_live_creators (guild_id, id)",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_go_live_enabled \
         ON go_live_creators (is_enabled, twitch_username)",
    )
    .execute(pool)
    .await?;

    println!("🔴 GoLive database initialized.");

    Ok(())
}
