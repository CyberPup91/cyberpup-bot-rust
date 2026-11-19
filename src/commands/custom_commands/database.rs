use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct CustomCommand {
    pub id: i64,
    pub guild_id: i64,
    pub name: String,
    pub response: String,
    pub cooldown: i64,
    pub user_cooldown: i64,
    pub allowed_roles: String,
    pub allowed_users: String,
    pub is_active: bool,
    pub counter: i64,
    pub should_reply: bool,
}

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS custom_commands (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            guild_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            response TEXT NOT NULL,
            cooldown INTEGER NOT NULL DEFAULT 5,
            user_cooldown INTEGER NOT NULL DEFAULT 0,
            allowed_roles TEXT NOT NULL DEFAULT '',
            allowed_users TEXT NOT NULL DEFAULT '',
            is_active BOOLEAN NOT NULL DEFAULT 1,
            counter INTEGER NOT NULL DEFAULT 0,
            should_reply BOOLEAN NOT NULL DEFAULT 0,
            UNIQUE(guild_id, name)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_custom_commands_guild_active
         ON custom_commands(guild_id, is_active)",
    )
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_command(
    pool: &SqlitePool,
    guild_id: u64,
    name: &str,
) -> Result<Option<CustomCommand>, sqlx::Error> {
    sqlx::query_as::<_, CustomCommand>(
        "SELECT * FROM custom_commands
         WHERE guild_id = ? AND lower(name) = lower(?) AND is_active = 1
         LIMIT 1",
    )
    .bind(guild_id as i64)
    .bind(name.trim_start_matches('!'))
    .fetch_optional(pool)
    .await
}

pub async fn increment_counter(
    pool: &SqlitePool,
    guild_id: u64,
    command_id: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "UPDATE custom_commands
         SET counter = counter + 1
         WHERE guild_id = ? AND id = ?
         RETURNING counter",
    )
    .bind(guild_id as i64)
    .bind(command_id)
    .fetch_one(pool)
    .await
}

pub async fn list_commands(
    pool: &SqlitePool,
    guild_id: u64,
) -> Result<Vec<CustomCommand>, sqlx::Error> {
    sqlx::query_as::<_, CustomCommand>(
        "SELECT * FROM custom_commands
         WHERE guild_id = ?
         ORDER BY name COLLATE NOCASE",
    )
    .bind(guild_id as i64)
    .fetch_all(pool)
    .await
}

pub async fn get_by_id(
    pool: &SqlitePool,
    guild_id: u64,
    command_id: i64,
) -> Result<Option<CustomCommand>, sqlx::Error> {
    sqlx::query_as::<_, CustomCommand>(
        "SELECT * FROM custom_commands
         WHERE guild_id = ? AND id = ?
         LIMIT 1",
    )
    .bind(guild_id as i64)
    .bind(command_id)
    .fetch_optional(pool)
    .await
}

pub async fn delete_command(
    pool: &SqlitePool,
    guild_id: u64,
    command_id: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM custom_commands WHERE guild_id = ? AND id = ?")
        .bind(guild_id as i64)
        .bind(command_id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}
