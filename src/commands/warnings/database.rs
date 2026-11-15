use sqlx::{Row, SqlitePool};

#[derive(Clone, Debug)]
pub struct WarningConfig {
    pub guild_id: u64,
    pub category_id: Option<u64>,
    pub staff_role_id: Option<u64>,
    pub mute_role_id: Option<u64>,
    pub log_channel_id: Option<u64>,
    pub warning_count: i64,
}

#[derive(Clone, Debug)]
pub struct WarningRecord {
    pub id: i64,
    pub guild_id: u64,
    pub user_id: u64,
    pub moderator_id: u64,
    pub channel_id: Option<u64>,
    pub title: String,
    pub explanation: String,
    pub require_staff_review: bool,
    pub status: String,
    pub acknowledged_at: Option<String>,
    pub resolved_by: Option<u64>,
    pub resolved_at: Option<String>,
    pub created_at: String,
}

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS warnings_config (guild_id INTEGER PRIMARY KEY, category_id INTEGER, staff_role_id INTEGER, mute_role_id INTEGER, log_channel_id INTEGER, warning_count INTEGER DEFAULT 0)")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS warnings (id INTEGER PRIMARY KEY AUTOINCREMENT, guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, moderator_id INTEGER NOT NULL, channel_id INTEGER, title TEXT NOT NULL, explanation TEXT NOT NULL, require_staff_review INTEGER DEFAULT 0, status TEXT DEFAULT 'active', acknowledged_at TIMESTAMP, resolved_by INTEGER, resolved_at TIMESTAMP, created_at TIMESTAMP NOT NULL)")
        .execute(pool).await?;
    Ok(())
}

fn id(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Option<u64>, sqlx::Error> {
    Ok(row
        .try_get::<Option<i64>, _>(column)?
        .and_then(|value| u64::try_from(value).ok()))
}

fn read_config(row: &sqlx::sqlite::SqliteRow) -> Result<WarningConfig, sqlx::Error> {
    Ok(WarningConfig {
        guild_id: row.try_get::<i64, _>("guild_id")? as u64,
        category_id: id(row, "category_id")?,
        staff_role_id: id(row, "staff_role_id")?,
        mute_role_id: id(row, "mute_role_id")?,
        log_channel_id: id(row, "log_channel_id")?,
        warning_count: row.try_get::<Option<i64>, _>("warning_count")?.unwrap_or(0),
    })
}

pub async fn get_config(
    pool: &SqlitePool,
    guild_id: u64,
) -> Result<Option<WarningConfig>, sqlx::Error> {
    sqlx::query("SELECT * FROM warnings_config WHERE guild_id = ?")
        .bind(guild_id as i64)
        .fetch_optional(pool)
        .await?
        .as_ref()
        .map(read_config)
        .transpose()
}

pub async fn set_config(
    pool: &SqlitePool,
    guild_id: u64,
    category_id: Option<u64>,
    staff_role_id: Option<u64>,
    mute_role_id: Option<u64>,
    log_channel_id: Option<u64>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO warnings_config (guild_id, category_id, staff_role_id, mute_role_id, log_channel_id) VALUES (?, ?, ?, ?, ?) ON CONFLICT(guild_id) DO UPDATE SET category_id = COALESCE(excluded.category_id, warnings_config.category_id), staff_role_id = COALESCE(excluded.staff_role_id, warnings_config.staff_role_id), mute_role_id = COALESCE(excluded.mute_role_id, warnings_config.mute_role_id), log_channel_id = COALESCE(excluded.log_channel_id, warnings_config.log_channel_id)")
        .bind(guild_id as i64).bind(category_id.map(|id| id as i64)).bind(staff_role_id.map(|id| id as i64)).bind(mute_role_id.map(|id| id as i64)).bind(log_channel_id.map(|id| id as i64)).execute(pool).await?;
    Ok(())
}

pub async fn next_warning_number(pool: &SqlitePool, guild_id: u64) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("INSERT INTO warnings_config (guild_id, warning_count) VALUES (?, 1) ON CONFLICT(guild_id) DO UPDATE SET warning_count = COALESCE(warnings_config.warning_count, 0) + 1 RETURNING warning_count")
        .bind(guild_id as i64).fetch_one(pool).await?;
    row.try_get("warning_count")
}

pub async fn create_warning(
    pool: &SqlitePool,
    guild_id: u64,
    user_id: u64,
    moderator_id: u64,
    channel_id: u64,
    title: &str,
    explanation: &str,
    require_staff_review: bool,
) -> Result<i64, sqlx::Error> {
    let result = sqlx::query("INSERT INTO warnings (guild_id, user_id, moderator_id, channel_id, title, explanation, require_staff_review, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'active', strftime('%Y-%m-%dT%H:%M:%fZ','now'))")
        .bind(guild_id as i64).bind(user_id as i64).bind(moderator_id as i64).bind(channel_id as i64).bind(title).bind(explanation).bind(i64::from(require_staff_review)).execute(pool).await?;
    Ok(result.last_insert_rowid())
}

fn read_warning(row: &sqlx::sqlite::SqliteRow) -> Result<WarningRecord, sqlx::Error> {
    Ok(WarningRecord {
        id: row.try_get("id")?,
        guild_id: row.try_get::<i64, _>("guild_id")? as u64,
        user_id: row.try_get::<i64, _>("user_id")? as u64,
        moderator_id: row.try_get::<i64, _>("moderator_id")? as u64,
        channel_id: id(row, "channel_id")?,
        title: row.try_get("title")?,
        explanation: row.try_get("explanation")?,
        require_staff_review: row
            .try_get::<Option<i64>, _>("require_staff_review")?
            .unwrap_or(0)
            != 0,
        status: row
            .try_get::<Option<String>, _>("status")?
            .unwrap_or_else(|| "active".into()),
        acknowledged_at: row.try_get("acknowledged_at")?,
        resolved_by: id(row, "resolved_by")?,
        resolved_at: row.try_get("resolved_at")?,
        created_at: row.try_get("created_at")?,
    })
}

pub async fn active_by_channel(
    pool: &SqlitePool,
    channel_id: u64,
) -> Result<Option<WarningRecord>, sqlx::Error> {
    sqlx::query("SELECT * FROM warnings WHERE channel_id = ? AND status != 'closed' ORDER BY id DESC LIMIT 1")
        .bind(channel_id as i64).fetch_optional(pool).await?.as_ref().map(read_warning).transpose()
}

pub async fn acknowledge(pool: &SqlitePool, warning_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE warnings SET acknowledged_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ? AND acknowledged_at IS NULL")
        .bind(warning_id).execute(pool).await?;
    Ok(())
}

pub async fn resolve(
    pool: &SqlitePool,
    warning_id: i64,
    moderator_id: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE warnings SET status = 'resolved', resolved_by = ?, resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ? AND status = 'active'")
        .bind(moderator_id as i64).bind(warning_id).execute(pool).await?;
    Ok(())
}

pub async fn close(pool: &SqlitePool, channel_id: u64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE warnings SET status = 'closed' WHERE channel_id = ?")
        .bind(channel_id as i64)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn history(
    pool: &SqlitePool,
    guild_id: u64,
    user_id: u64,
) -> Result<Vec<WarningRecord>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT * FROM warnings WHERE guild_id = ? AND user_id = ? ORDER BY id DESC LIMIT 10",
    )
    .bind(guild_id as i64)
    .bind(user_id as i64)
    .fetch_all(pool)
    .await?;
    rows.iter().map(read_warning).collect()
}
