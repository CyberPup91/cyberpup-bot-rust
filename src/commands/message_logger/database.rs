#[derive(Debug, Clone)]
pub struct UserLogSettings {
    pub log_channel_id: Option<u64>,
    pub log_all: bool,
    pub log_edited: bool,
    pub log_deleted: bool,
}

#[derive(Debug, Clone)]
pub struct MessageSnapshot {
    pub guild_id: u64,
    pub user_id: u64,
    pub author_name: String,
    pub avatar_url: String,
    pub content: String,
    pub attachment_names: Vec<String>,
    pub created_at: String,
}

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("CREATE TABLE IF NOT EXISTS user_log_settings (guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, log_channel_id INTEGER, log_all INTEGER NOT NULL DEFAULT 0, log_edited INTEGER NOT NULL DEFAULT 0, log_deleted INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (guild_id, user_id))")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS message_snapshots (channel_id INTEGER NOT NULL, message_id INTEGER NOT NULL, guild_id INTEGER NOT NULL, user_id INTEGER NOT NULL, author_name TEXT NOT NULL, avatar_url TEXT NOT NULL, content TEXT NOT NULL, attachment_names TEXT NOT NULL, created_at TEXT NOT NULL, PRIMARY KEY (channel_id, message_id))")
        .execute(pool).await?;
    Ok(())
}

pub async fn save_snapshot(
    pool: &sqlx::SqlitePool,
    channel_id: u64,
    message_id: u64,
    snapshot: &MessageSnapshot,
) -> Result<(), sqlx::Error> {
    let attachments = serde_json::to_string(&snapshot.attachment_names).unwrap_or_default();
    sqlx::query("INSERT INTO message_snapshots (channel_id,message_id,guild_id,user_id,author_name,avatar_url,content,attachment_names,created_at) VALUES (?,?,?,?,?,?,?,?,?) ON CONFLICT(channel_id,message_id) DO UPDATE SET guild_id=excluded.guild_id,user_id=excluded.user_id,author_name=excluded.author_name,avatar_url=excluded.avatar_url,content=excluded.content,attachment_names=excluded.attachment_names,created_at=excluded.created_at")
        .bind(channel_id as i64).bind(message_id as i64).bind(snapshot.guild_id as i64).bind(snapshot.user_id as i64)
        .bind(&snapshot.author_name).bind(&snapshot.avatar_url).bind(&snapshot.content).bind(attachments).bind(&snapshot.created_at)
        .execute(pool).await?;
    Ok(())
}

pub async fn get_snapshot(
    pool: &sqlx::SqlitePool,
    channel_id: u64,
    message_id: u64,
) -> Result<Option<MessageSnapshot>, sqlx::Error> {
    let row = sqlx::query("SELECT guild_id,user_id,author_name,avatar_url,content,attachment_names,created_at FROM message_snapshots WHERE channel_id=? AND message_id=?")
        .bind(channel_id as i64).bind(message_id as i64).fetch_optional(pool).await?;
    Ok(row.map(|row| {
        let attachment_json: String =
            sqlx::Row::try_get(&row, "attachment_names").unwrap_or_default();
        MessageSnapshot {
            guild_id: sqlx::Row::try_get::<i64, _>(&row, "guild_id").unwrap_or_default() as u64,
            user_id: sqlx::Row::try_get::<i64, _>(&row, "user_id").unwrap_or_default() as u64,
            author_name: sqlx::Row::try_get(&row, "author_name").unwrap_or_default(),
            avatar_url: sqlx::Row::try_get(&row, "avatar_url").unwrap_or_default(),
            content: sqlx::Row::try_get(&row, "content").unwrap_or_default(),
            attachment_names: serde_json::from_str(&attachment_json).unwrap_or_default(),
            created_at: sqlx::Row::try_get(&row, "created_at").unwrap_or_default(),
        }
    }))
}

pub async fn update_snapshot_content(
    pool: &sqlx::SqlitePool,
    channel_id: u64,
    message_id: u64,
    content: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE message_snapshots SET content=? WHERE channel_id=? AND message_id=?")
        .bind(content)
        .bind(channel_id as i64)
        .bind(message_id as i64)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_snapshot(
    pool: &sqlx::SqlitePool,
    channel_id: u64,
    message_id: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM message_snapshots WHERE channel_id=? AND message_id=?")
        .bind(channel_id as i64)
        .bind(message_id as i64)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get(
    pool: &sqlx::SqlitePool,
    guild_id: u64,
    user_id: u64,
) -> Result<Option<UserLogSettings>, sqlx::Error> {
    let row = sqlx::query("SELECT log_channel_id, log_all, log_edited, log_deleted FROM user_log_settings WHERE guild_id = ? AND user_id = ?")
        .bind(guild_id as i64).bind(user_id as i64).fetch_optional(pool).await?;
    Ok(row.map(|row| UserLogSettings {
        log_channel_id: sqlx::Row::try_get::<Option<i64>, _>(&row, "log_channel_id")
            .ok()
            .flatten()
            .map(|v| v as u64),
        log_all: sqlx::Row::try_get::<i64, _>(&row, "log_all").unwrap_or_default() != 0,
        log_edited: sqlx::Row::try_get::<i64, _>(&row, "log_edited").unwrap_or_default() != 0,
        log_deleted: sqlx::Row::try_get::<i64, _>(&row, "log_deleted").unwrap_or_default() != 0,
    }))
}

pub async fn save(
    pool: &sqlx::SqlitePool,
    guild_id: u64,
    user_id: u64,
    channel_id: u64,
    log_all: bool,
    log_edited: bool,
    log_deleted: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO user_log_settings (guild_id,user_id,log_channel_id,log_all,log_edited,log_deleted) VALUES (?,?,?,?,?,?) ON CONFLICT(guild_id,user_id) DO UPDATE SET log_channel_id=excluded.log_channel_id, log_all=excluded.log_all, log_edited=excluded.log_edited, log_deleted=excluded.log_deleted")
        .bind(guild_id as i64).bind(user_id as i64).bind(channel_id as i64).bind(log_all as i64).bind(log_edited as i64).bind(log_deleted as i64).execute(pool).await?;
    Ok(())
}

pub async fn remove(
    pool: &sqlx::SqlitePool,
    guild_id: u64,
    user_id: u64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM user_log_settings WHERE guild_id = ? AND user_id = ?")
        .bind(guild_id as i64)
        .bind(user_id as i64)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
