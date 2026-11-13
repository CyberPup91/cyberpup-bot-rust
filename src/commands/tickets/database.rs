use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct TicketConfig {
    pub guild_id: u64,
    pub category_id: Option<u64>,
    pub support_role_id: Option<u64>,
    pub log_channel_id: Option<u64>,
    pub ticket_count: i64,
    pub block_role_id: Option<u64>,
    pub panel_title: String,
    pub panel_description: String,
    pub panel_footer: String,
    pub max_user_tickets: i64,
    pub ticket_welcome_msg: String,
}

#[derive(Debug, Clone)]
pub struct TicketOption {
    pub id: i64,
    pub label: String,
    pub emoji: Option<String>,
    pub description: Option<String>,
    pub support_role_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Ticket {
    pub channel_id: u64,
    pub guild_id: u64,
    pub creator_id: u64,
    pub status: String,
    pub claimed_by: Option<u64>,
    pub support_role_id: Option<u64>,
}

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS ticket_config (
            guild_id INTEGER PRIMARY KEY,
            category_id INTEGER,
            support_role_id INTEGER,
            log_channel_id INTEGER,
            ticket_count INTEGER DEFAULT 0,
            block_role_id INTEGER,
            panel_title TEXT DEFAULT '📩 Open a Support Ticket',
            panel_description TEXT DEFAULT 'Need assistance or have a question for our support team? Select a category or click below to open a private ticket.',
            panel_footer TEXT DEFAULT 'Support Network',
            max_user_tickets INTEGER DEFAULT 1,
            ticket_welcome_msg TEXT DEFAULT 'Welcome {user}! Please explain your request or issue below in detail.'
        )"#,
    )
    .execute(pool)
    .await?;

    // Keep older installations usable by adding columns introduced by later cog versions.
    let config_columns = table_columns(pool, "ticket_config").await?;
    let migrations = [
        ("block_role_id", "INTEGER"),
        ("panel_title", "TEXT DEFAULT '📩 Open a Support Ticket'"),
        (
            "panel_description",
            "TEXT DEFAULT 'Select a category to open a ticket.'",
        ),
        ("panel_footer", "TEXT DEFAULT 'Support Network'"),
        ("max_user_tickets", "INTEGER DEFAULT 1"),
        (
            "ticket_welcome_msg",
            "TEXT DEFAULT 'Welcome {user}! Please explain your request.'",
        ),
        ("ticket_count", "INTEGER DEFAULT 0"),
    ];
    for (column, definition) in migrations {
        if !config_columns.iter().any(|name| name == column) {
            sqlx::query(&format!(
                "ALTER TABLE ticket_config ADD COLUMN {column} {definition}"
            ))
            .execute(pool)
            .await?;
        }
    }

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS ticket_options (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            guild_id INTEGER,
            label TEXT,
            emoji TEXT,
            description TEXT,
            support_role_id INTEGER
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS tickets (
            channel_id INTEGER PRIMARY KEY,
            guild_id INTEGER,
            creator_id INTEGER,
            status TEXT,
            claimed_by INTEGER,
            support_role_id INTEGER,
            created_at TIMESTAMP
        )"#,
    )
    .execute(pool)
    .await?;

    let ticket_columns = table_columns(pool, "tickets").await?;
    for (column, definition) in [("claimed_by", "INTEGER"), ("support_role_id", "INTEGER")] {
        if !ticket_columns.iter().any(|name| name == column) {
            sqlx::query(&format!(
                "ALTER TABLE tickets ADD COLUMN {column} {definition}"
            ))
            .execute(pool)
            .await?;
        }
    }

    Ok(())
}

async fn table_columns(pool: &SqlitePool, table: &str) -> Result<Vec<String>, sqlx::Error> {
    let query = format!("PRAGMA table_info({table})");
    Ok(sqlx::query(&query)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect())
}

fn optional_id(row: &sqlx::sqlite::SqliteRow, key: &str) -> Result<Option<u64>, sqlx::Error> {
    Ok(row.try_get::<Option<i64>, _>(key)?.map(|id| id as u64))
}

pub async fn get_config(
    pool: &SqlitePool,
    guild_id: u64,
) -> Result<Option<TicketConfig>, sqlx::Error> {
    let row = sqlx::query("SELECT * FROM ticket_config WHERE guild_id = ?")
        .bind(guild_id as i64)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };

    Ok(Some(TicketConfig {
        guild_id,
        category_id: optional_id(&row, "category_id")?,
        support_role_id: optional_id(&row, "support_role_id")?,
        log_channel_id: optional_id(&row, "log_channel_id")?,
        ticket_count: row.try_get::<Option<i64>, _>("ticket_count")?.unwrap_or(0),
        block_role_id: optional_id(&row, "block_role_id")?,
        panel_title: row
            .try_get::<Option<String>, _>("panel_title")?
            .unwrap_or_else(|| "📩 Open a Support Ticket".into()),
        panel_description: row
            .try_get::<Option<String>, _>("panel_description")?
            .unwrap_or_else(|| "Select a category to open a ticket.".into()),
        panel_footer: row
            .try_get::<Option<String>, _>("panel_footer")?
            .unwrap_or_else(|| "Support Network".into()),
        max_user_tickets: row
            .try_get::<Option<i64>, _>("max_user_tickets")?
            .unwrap_or(1)
            .max(1),
        ticket_welcome_msg: row
            .try_get::<Option<String>, _>("ticket_welcome_msg")?
            .unwrap_or_else(|| "Welcome {user}! Please explain your request.".into()),
    }))
}

pub async fn ensure_config(pool: &SqlitePool, guild_id: u64) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO ticket_config (guild_id) VALUES (?)")
        .bind(guild_id as i64)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn save_config(pool: &SqlitePool, config: &TicketConfig) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO ticket_config (
            guild_id, category_id, support_role_id, log_channel_id, ticket_count,
            block_role_id, panel_title, panel_description, panel_footer,
            max_user_tickets, ticket_welcome_msg
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(guild_id) DO UPDATE SET
            category_id=excluded.category_id, support_role_id=excluded.support_role_id,
            log_channel_id=excluded.log_channel_id, block_role_id=excluded.block_role_id,
            panel_title=excluded.panel_title, panel_description=excluded.panel_description,
            panel_footer=excluded.panel_footer, max_user_tickets=excluded.max_user_tickets,
            ticket_welcome_msg=excluded.ticket_welcome_msg"#,
    )
    .bind(config.guild_id as i64)
    .bind(config.category_id.map(|id| id as i64))
    .bind(config.support_role_id.map(|id| id as i64))
    .bind(config.log_channel_id.map(|id| id as i64))
    .bind(config.ticket_count)
    .bind(config.block_role_id.map(|id| id as i64))
    .bind(&config.panel_title)
    .bind(&config.panel_description)
    .bind(&config.panel_footer)
    .bind(config.max_user_tickets.max(1))
    .bind(&config.ticket_welcome_msg)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_options(
    pool: &SqlitePool,
    guild_id: u64,
) -> Result<Vec<TicketOption>, sqlx::Error> {
    let rows = sqlx::query("SELECT * FROM ticket_options WHERE guild_id = ? ORDER BY id")
        .bind(guild_id as i64)
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|row| {
            Ok(TicketOption {
                id: row.try_get("id")?,
                label: row
                    .try_get::<Option<String>, _>("label")?
                    .unwrap_or_else(|| "Support".into()),
                emoji: row.try_get::<Option<String>, _>("emoji")?,
                description: row.try_get::<Option<String>, _>("description")?,
                support_role_id: optional_id(row, "support_role_id")?,
            })
        })
        .collect()
}

pub async fn get_option(
    pool: &SqlitePool,
    guild_id: u64,
    option_id: i64,
) -> Result<Option<TicketOption>, sqlx::Error> {
    Ok(list_options(pool, guild_id)
        .await?
        .into_iter()
        .find(|option| option.id == option_id))
}

pub async fn add_option(
    pool: &SqlitePool,
    guild_id: u64,
    label: &str,
    emoji: Option<&str>,
    description: Option<&str>,
    support_role_id: Option<u64>,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO ticket_options (guild_id, label, emoji, description, support_role_id) VALUES (?, ?, ?, ?, ?)")
        .bind(guild_id as i64).bind(label).bind(emoji).bind(description).bind(support_role_id.map(|id| id as i64)).execute(pool).await?;
    Ok(())
}

pub async fn delete_option(
    pool: &SqlitePool,
    guild_id: u64,
    option_id: i64,
) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM ticket_options WHERE id = ? AND guild_id = ?")
            .bind(option_id)
            .bind(guild_id as i64)
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

pub async fn next_ticket_number(pool: &SqlitePool, guild_id: u64) -> Result<i64, sqlx::Error> {
    ensure_config(pool, guild_id).await?;
    let row = sqlx::query("UPDATE ticket_config SET ticket_count = COALESCE(ticket_count, 0) + 1 WHERE guild_id = ? RETURNING ticket_count")
        .bind(guild_id as i64).fetch_one(pool).await?;
    row.try_get("ticket_count")
}

pub async fn open_ticket_count(
    pool: &SqlitePool,
    guild_id: u64,
    creator_id: u64,
) -> Result<i64, sqlx::Error> {
    sqlx::query("SELECT COUNT(*) AS count FROM tickets WHERE guild_id = ? AND creator_id = ? AND status = 'open'")
        .bind(guild_id as i64).bind(creator_id as i64).fetch_one(pool).await?.try_get("count")
}

pub async fn add_ticket(
    pool: &SqlitePool,
    channel_id: u64,
    guild_id: u64,
    creator_id: u64,
    support_role_id: u64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO tickets (channel_id, guild_id, creator_id, status, support_role_id, created_at) VALUES (?, ?, ?, 'open', ?, datetime('now'))")
        .bind(channel_id as i64).bind(guild_id as i64).bind(creator_id as i64).bind(support_role_id as i64).execute(pool).await?;
    Ok(())
}

pub async fn get_ticket(pool: &SqlitePool, channel_id: u64) -> Result<Option<Ticket>, sqlx::Error> {
    let row = sqlx::query("SELECT * FROM tickets WHERE channel_id = ?")
        .bind(channel_id as i64)
        .fetch_optional(pool)
        .await?;
    row.map(|row| {
        Ok(Ticket {
            channel_id: row.try_get::<i64, _>("channel_id")? as u64,
            guild_id: row.try_get::<i64, _>("guild_id")? as u64,
            creator_id: row.try_get::<i64, _>("creator_id")? as u64,
            status: row
                .try_get::<Option<String>, _>("status")?
                .unwrap_or_else(|| "open".into()),
            claimed_by: optional_id(&row, "claimed_by")?,
            support_role_id: optional_id(&row, "support_role_id")?,
        })
    })
    .transpose()
}

pub async fn claim_ticket(
    pool: &SqlitePool,
    channel_id: u64,
    user_id: u64,
) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("UPDATE tickets SET claimed_by = ? WHERE channel_id = ? AND status = 'open' AND claimed_by IS NULL")
        .bind(user_id as i64).bind(channel_id as i64).execute(pool).await?.rows_affected() > 0)
}

pub async fn close_ticket(pool: &SqlitePool, channel_id: u64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE tickets SET status = 'closed' WHERE channel_id = ?")
        .bind(channel_id as i64)
        .execute(pool)
        .await?;
    Ok(())
}
