use sqlx::{Row, SqlitePool};

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS greeter_config (
            guild_id INTEGER PRIMARY KEY,
            join_channel_id INTEGER,
            leave_channel_id INTEGER,
            join_enabled INTEGER DEFAULT 1,
            leave_enabled INTEGER DEFAULT 1,
            join_title TEXT DEFAULT '👋 Welcome!',
            join_description TEXT DEFAULT 'Welcome to the server, {user}!',
            join_footer TEXT DEFAULT 'We''re glad you''re here!',
            join_color TEXT DEFAULT 'purple',
            leave_title TEXT DEFAULT '👋 See You Later!',
            leave_description TEXT DEFAULT 'Goodbye, {user}!',
            leave_footer TEXT DEFAULT 'We''ll miss you!',
            leave_color TEXT DEFAULT 'red'
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Migrate older databases safely by adding any missing columns.
    let columns = sqlx::query("PRAGMA table_info(greeter_config)")
        .fetch_all(pool)
        .await?;

    let existing: Vec<String> = columns
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();

    let migrations = [
        ("join_channel_id", "INTEGER"),
        ("leave_channel_id", "INTEGER"),
        ("join_enabled", "INTEGER DEFAULT 1"),
        ("leave_enabled", "INTEGER DEFAULT 1"),
        ("join_title", "TEXT DEFAULT '👋 Welcome!'"),
        (
            "join_description",
            "TEXT DEFAULT 'Welcome to the server, {user}!'",
        ),
        ("join_footer", "TEXT DEFAULT 'We''re glad you''re here!'"),
        ("join_color", "TEXT DEFAULT 'purple'"),
        ("leave_title", "TEXT DEFAULT '👋 See You Later!'"),
        ("leave_description", "TEXT DEFAULT 'Goodbye, {user}!'"),
        ("leave_footer", "TEXT DEFAULT 'We''ll miss you!'"),
        ("leave_color", "TEXT DEFAULT 'red'"),
    ];

    for (column, definition) in migrations {
        if !existing.iter().any(|name| name == column) {
            let query = format!("ALTER TABLE greeter_config ADD COLUMN {column} {definition}");

            sqlx::query(&query).execute(pool).await?;
        }
    }

    println!("👋 Greeter database initialized.");

    Ok(())
}

#[derive(Debug, Clone)]
pub struct GreeterConfig {
    pub guild_id: u64,
    pub join_channel_id: Option<u64>,
    pub leave_channel_id: Option<u64>,
    pub join_enabled: bool,
    pub leave_enabled: bool,
    pub join_title: String,
    pub join_description: String,
    pub join_footer: String,
    pub join_color: String,
    pub leave_title: String,
    pub leave_description: String,
    pub leave_footer: String,
    pub leave_color: String,
}

impl Default for GreeterConfig {
    fn default() -> Self {
        Self {
            guild_id: 0,
            join_channel_id: None,
            leave_channel_id: None,
            join_enabled: true,
            leave_enabled: true,
            join_title: "👋 Welcome!".into(),
            join_description: "Welcome to the server, {user}!".into(),
            join_footer: "We're glad you're here!".into(),
            join_color: "purple".into(),
            leave_title: "👋 See You Later!".into(),
            leave_description: "Goodbye, {user}!".into(),
            leave_footer: "We'll miss you!".into(),
            leave_color: "red".into(),
        }
    }
}

pub async fn get_config(pool: &SqlitePool, guild_id: u64) -> Result<GreeterConfig, sqlx::Error> {
    let guild_id_i64 = guild_id as i64;

    let row = sqlx::query("SELECT * FROM greeter_config WHERE guild_id = ?")
        .bind(guild_id_i64)
        .fetch_optional(pool)
        .await?;

    let Some(row) = row else {
        let config = GreeterConfig {
            guild_id,
            ..Default::default()
        };

        save_config(pool, &config).await?;
        return Ok(config);
    };

    Ok(GreeterConfig {
        guild_id,
        join_channel_id: row
            .try_get::<Option<i64>, _>("join_channel_id")?
            .map(|id| id as u64),
        leave_channel_id: row
            .try_get::<Option<i64>, _>("leave_channel_id")?
            .map(|id| id as u64),
        join_enabled: row.try_get::<Option<i64>, _>("join_enabled")?.unwrap_or(1) != 0,
        leave_enabled: row.try_get::<Option<i64>, _>("leave_enabled")?.unwrap_or(1) != 0,
        join_title: row
            .try_get::<Option<String>, _>("join_title")?
            .unwrap_or_else(|| "👋 Welcome!".into()),
        join_description: row
            .try_get::<Option<String>, _>("join_description")?
            .unwrap_or_else(|| "Welcome to the server, {user}!".into()),
        join_footer: row
            .try_get::<Option<String>, _>("join_footer")?
            .unwrap_or_else(|| "We're glad you're here!".into()),
        join_color: row
            .try_get::<Option<String>, _>("join_color")?
            .unwrap_or_else(|| "purple".into()),
        leave_title: row
            .try_get::<Option<String>, _>("leave_title")?
            .unwrap_or_else(|| "👋 See You Later!".into()),
        leave_description: row
            .try_get::<Option<String>, _>("leave_description")?
            .unwrap_or_else(|| "Goodbye, {user}!".into()),
        leave_footer: row
            .try_get::<Option<String>, _>("leave_footer")?
            .unwrap_or_else(|| "We'll miss you!".into()),
        leave_color: row
            .try_get::<Option<String>, _>("leave_color")?
            .unwrap_or_else(|| "red".into()),
    })
}

pub async fn save_config(pool: &SqlitePool, config: &GreeterConfig) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO greeter_config (
            guild_id, join_channel_id, leave_channel_id,
            join_enabled, leave_enabled,
            join_title, join_description, join_footer, join_color,
            leave_title, leave_description, leave_footer, leave_color
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(guild_id) DO UPDATE SET
            join_channel_id = excluded.join_channel_id,
            leave_channel_id = excluded.leave_channel_id,
            join_enabled = excluded.join_enabled,
            leave_enabled = excluded.leave_enabled,
            join_title = excluded.join_title,
            join_description = excluded.join_description,
            join_footer = excluded.join_footer,
            join_color = excluded.join_color,
            leave_title = excluded.leave_title,
            leave_description = excluded.leave_description,
            leave_footer = excluded.leave_footer,
            leave_color = excluded.leave_color
        "#,
    )
    .bind(config.guild_id as i64)
    .bind(config.join_channel_id.map(|id| id as i64))
    .bind(config.leave_channel_id.map(|id| id as i64))
    .bind(config.join_enabled as i64)
    .bind(config.leave_enabled as i64)
    .bind(&config.join_title)
    .bind(&config.join_description)
    .bind(&config.join_footer)
    .bind(&config.join_color)
    .bind(&config.leave_title)
    .bind(&config.leave_description)
    .bind(&config.leave_footer)
    .bind(&config.leave_color)
    .execute(pool)
    .await?;

    Ok(())
}
