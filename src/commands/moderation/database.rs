use sqlx::{Row, SqlitePool};

#[derive(Clone, Debug)]
pub struct StickySnapshot {
    pub role_ids: Vec<u64>,
    pub timestamp: i64,
}

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS mod_settings (guild_id INTEGER PRIMARY KEY, log_channel_id INTEGER)")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS sticky_roles_config (guild_id INTEGER PRIMARY KEY, enabled INTEGER DEFAULT 0)")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS sticky_blacklist (guild_id INTEGER NOT NULL, role_id INTEGER NOT NULL, PRIMARY KEY (guild_id, role_id))")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS sticky_roles_snapshots (guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, role_ids TEXT NOT NULL, timestamp INTEGER NOT NULL, PRIMARY KEY (guild_id, user_id))")
        .execute(pool).await?;
    Ok(())
}

pub async fn log_channel(pool: &SqlitePool, guild_id: u64) -> Result<Option<u64>, sqlx::Error> {
    let raw: Option<i64> =
        sqlx::query_scalar("SELECT log_channel_id FROM mod_settings WHERE guild_id = ?")
            .bind(guild_id as i64)
            .fetch_optional(pool)
            .await?
            .flatten();
    Ok(raw.and_then(|id| u64::try_from(id).ok()))
}

pub async fn set_log_channel(
    pool: &SqlitePool,
    guild_id: u64,
    channel_id: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO mod_settings (guild_id, log_channel_id) VALUES (?, ?) ON CONFLICT(guild_id) DO UPDATE SET log_channel_id = excluded.log_channel_id")
        .bind(guild_id as i64).bind(channel_id as i64).execute(pool).await?;
    Ok(())
}

pub async fn sticky_enabled(pool: &SqlitePool, guild_id: u64) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, Option<i64>>(
        "SELECT enabled FROM sticky_roles_config WHERE guild_id = ?",
    )
    .bind(guild_id as i64)
    .fetch_optional(pool)
    .await?
    .flatten()
    .unwrap_or(0)
        != 0)
}

pub async fn set_sticky_enabled(
    pool: &SqlitePool,
    guild_id: u64,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO sticky_roles_config (guild_id, enabled) VALUES (?, ?) ON CONFLICT(guild_id) DO UPDATE SET enabled = excluded.enabled")
        .bind(guild_id as i64).bind(i64::from(enabled)).execute(pool).await?;
    Ok(())
}

pub async fn blacklisted_roles(pool: &SqlitePool, guild_id: u64) -> Result<Vec<u64>, sqlx::Error> {
    let rows =
        sqlx::query("SELECT role_id FROM sticky_blacklist WHERE guild_id = ? ORDER BY role_id")
            .bind(guild_id as i64)
            .fetch_all(pool)
            .await?;
    rows.iter()
        .map(|row| Ok(row.try_get::<i64, _>("role_id")? as u64))
        .collect()
}

pub async fn blacklist_role(
    pool: &SqlitePool,
    guild_id: u64,
    role_id: u64,
    exclude: bool,
) -> Result<(), sqlx::Error> {
    if exclude {
        sqlx::query("INSERT OR IGNORE INTO sticky_blacklist (guild_id, role_id) VALUES (?, ?)")
            .bind(guild_id as i64)
            .bind(role_id as i64)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("DELETE FROM sticky_blacklist WHERE guild_id = ? AND role_id = ?")
            .bind(guild_id as i64)
            .bind(role_id as i64)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn snapshot(
    pool: &SqlitePool,
    guild_id: u64,
    user_id: u64,
    roles: &[u64],
) -> Result<(), sqlx::Error> {
    if roles.is_empty() {
        return Ok(());
    }
    let role_ids = roles
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    sqlx::query("INSERT INTO sticky_roles_snapshots (guild_id, user_id, role_ids, timestamp) VALUES (?, ?, ?, unixepoch()) ON CONFLICT(guild_id, user_id) DO UPDATE SET role_ids = excluded.role_ids, timestamp = excluded.timestamp")
        .bind(guild_id as i64).bind(user_id as i64).bind(role_ids).execute(pool).await?;
    Ok(())
}

pub async fn get_snapshot(
    pool: &SqlitePool,
    guild_id: u64,
    user_id: u64,
) -> Result<Option<StickySnapshot>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT role_ids, timestamp FROM sticky_roles_snapshots WHERE guild_id = ? AND user_id = ?",
    )
    .bind(guild_id as i64)
    .bind(user_id as i64)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let raw: String = row.try_get("role_ids")?;
        let role_ids = raw
            .split(',')
            .filter_map(|item| item.parse().ok())
            .collect();
        Ok(StickySnapshot {
            role_ids,
            timestamp: row.try_get("timestamp")?,
        })
    })
    .transpose()
}

pub async fn remove_snapshot(
    pool: &SqlitePool,
    guild_id: u64,
    user_id: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sticky_roles_snapshots WHERE guild_id = ? AND user_id = ?")
        .bind(guild_id as i64)
        .bind(user_id as i64)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn snapshot_count(pool: &SqlitePool, guild_id: u64) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT COUNT(*) FROM sticky_roles_snapshots WHERE guild_id = ?")
        .bind(guild_id as i64)
        .fetch_one(pool)
        .await
}
