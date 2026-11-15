use sqlx::{Row, SqlitePool};

#[derive(Clone)]
pub struct GuildConfig {
    pub channel_id: Option<u64>,
    pub current_count: i64,
    pub last_user_id: Option<u64>,
    pub role_id: Option<u64>,
    pub reset_on_fail: bool,
    pub penalty_amount: i64,
    pub base_reward: i64,
    pub math_bonus: i64,
    pub currency_name: String,
    pub currency_emoji: String,
    pub leaderboard_channel_id: Option<u64>,
    pub leaderboard_message_id: Option<u64>,
}

pub struct UserStats {
    pub user_id: u64,
    pub balance: i64,
    pub counts_made: i64,
    pub math_solved: i64,
}

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("PRAGMA journal_mode=WAL").execute(pool).await?;
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS guild_config (
            guild_id INTEGER PRIMARY KEY,
            channel_id INTEGER DEFAULT NULL,
            current_count INTEGER DEFAULT 0,
            last_user_id INTEGER DEFAULT NULL,
            role_id INTEGER DEFAULT NULL,
            reset_on_fail INTEGER DEFAULT 1,
            penalty_amount INTEGER DEFAULT 10,
            base_reward INTEGER DEFAULT 1,
            math_bonus INTEGER DEFAULT 5,
            currency_name TEXT DEFAULT 'Points',
            currency_emoji TEXT DEFAULT '🪙',
            last_topic_update INTEGER DEFAULT 0,
            leaderboard_channel_id INTEGER DEFAULT NULL,
            leaderboard_message_id INTEGER DEFAULT NULL,
            last_leaderboard_update INTEGER DEFAULT 0
        )"#,
    )
    .execute(pool)
    .await?;
    let columns = table_columns(pool, "guild_config").await?;
    for (name, definition) in [
        ("channel_id", "INTEGER DEFAULT NULL"),
        ("current_count", "INTEGER DEFAULT 0"),
        ("last_user_id", "INTEGER DEFAULT NULL"),
        ("role_id", "INTEGER DEFAULT NULL"),
        ("reset_on_fail", "INTEGER DEFAULT 1"),
        ("penalty_amount", "INTEGER DEFAULT 10"),
        ("base_reward", "INTEGER DEFAULT 1"),
        ("math_bonus", "INTEGER DEFAULT 5"),
        ("currency_name", "TEXT DEFAULT 'Points'"),
        ("currency_emoji", "TEXT DEFAULT '🪙'"),
        ("last_topic_update", "INTEGER DEFAULT 0"),
        ("leaderboard_channel_id", "INTEGER DEFAULT NULL"),
        ("leaderboard_message_id", "INTEGER DEFAULT NULL"),
        ("last_leaderboard_update", "INTEGER DEFAULT 0"),
    ] {
        if !columns.iter().any(|column| column == name) {
            sqlx::query(&format!(
                "ALTER TABLE guild_config ADD COLUMN {name} {definition}"
            ))
            .execute(pool)
            .await?;
        }
    }
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS users (
            guild_id INTEGER NOT NULL,
            user_id INTEGER NOT NULL,
            balance INTEGER DEFAULT 0,
            counts_made INTEGER DEFAULT 0,
            math_solved INTEGER DEFAULT 0,
            PRIMARY KEY (guild_id, user_id)
        )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn table_columns(pool: &SqlitePool, table: &str) -> Result<Vec<String>, sqlx::Error> {
    Ok(sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect())
}

fn optional_id(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Option<u64>, sqlx::Error> {
    Ok(row
        .try_get::<Option<i64>, _>(column)?
        .and_then(|id| u64::try_from(id).ok()))
}

pub async fn get_config(pool: &SqlitePool, guild_id: u64) -> Result<GuildConfig, sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO guild_config (guild_id) VALUES (?)")
        .bind(guild_id as i64)
        .execute(pool)
        .await?;
    let row = sqlx::query("SELECT * FROM guild_config WHERE guild_id = ?")
        .bind(guild_id as i64)
        .fetch_one(pool)
        .await?;
    Ok(GuildConfig {
        channel_id: optional_id(&row, "channel_id")?,
        current_count: row.try_get::<Option<i64>, _>("current_count")?.unwrap_or(0),
        last_user_id: optional_id(&row, "last_user_id")?,
        role_id: optional_id(&row, "role_id")?,
        reset_on_fail: row.try_get::<Option<i64>, _>("reset_on_fail")?.unwrap_or(1) != 0,
        penalty_amount: row
            .try_get::<Option<i64>, _>("penalty_amount")?
            .unwrap_or(10),
        base_reward: row.try_get::<Option<i64>, _>("base_reward")?.unwrap_or(1),
        math_bonus: row.try_get::<Option<i64>, _>("math_bonus")?.unwrap_or(5),
        currency_name: row
            .try_get::<Option<String>, _>("currency_name")?
            .unwrap_or_else(|| "Points".into()),
        currency_emoji: row
            .try_get::<Option<String>, _>("currency_emoji")?
            .unwrap_or_else(|| "🪙".into()),
        leaderboard_channel_id: optional_id(&row, "leaderboard_channel_id")?,
        leaderboard_message_id: optional_id(&row, "leaderboard_message_id")?,
    })
}

pub async fn update_user_result(
    pool: &SqlitePool,
    guild_id: u64,
    user_id: u64,
    delta: i64,
    successful: bool,
    math: bool,
) -> Result<(), sqlx::Error> {
    // The Python cog counts every numeric attempt, including incorrect ones.
    let made_count = 1_i64;
    let solved_math = i64::from(successful && math);
    sqlx::query(
        r#"INSERT INTO users (guild_id, user_id, balance, counts_made, math_solved)
        VALUES (?, ?, MAX(0, ?), ?, ?)
        ON CONFLICT(guild_id, user_id) DO UPDATE SET
            balance = MAX(0, balance + ?),
            counts_made = counts_made + excluded.counts_made,
            math_solved = math_solved + excluded.math_solved"#,
    )
    .bind(guild_id as i64)
    .bind(user_id as i64)
    .bind(delta)
    .bind(made_count)
    .bind(solved_math)
    .bind(delta)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn top_users(
    pool: &SqlitePool,
    guild_id: u64,
    limit: i64,
) -> Result<Vec<UserStats>, sqlx::Error> {
    let rows = sqlx::query("SELECT user_id, balance, counts_made, math_solved FROM users WHERE guild_id = ? ORDER BY balance DESC LIMIT ?")
        .bind(guild_id as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    rows.iter()
        .map(|row| {
            Ok(UserStats {
                user_id: row.try_get::<i64, _>("user_id")? as u64,
                balance: row.try_get::<Option<i64>, _>("balance")?.unwrap_or(0),
                counts_made: row.try_get::<Option<i64>, _>("counts_made")?.unwrap_or(0),
                math_solved: row.try_get::<Option<i64>, _>("math_solved")?.unwrap_or(0),
            })
        })
        .collect()
}
