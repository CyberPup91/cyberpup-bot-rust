use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PanelBinding {
    pub max_selections: u8,
}

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("CREATE TABLE IF NOT EXISTS reaction_role_panels (message_id INTEGER PRIMARY KEY, guild_id INTEGER, channel_id INTEGER, max_selections INTEGER NOT NULL DEFAULT 0, panel_name TEXT)")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS reaction_role_bindings (message_id INTEGER NOT NULL, emoji TEXT NOT NULL, role_id INTEGER NOT NULL, PRIMARY KEY (message_id, emoji))")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS reaction_role_migrations (name TEXT PRIMARY KEY, completed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .execute(pool).await?;
    migrate_legacy_json(pool).await?;
    Ok(())
}

async fn migrate_legacy_json(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let migrated: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM reaction_role_migrations WHERE name='reaction_roles_json'",
    )
    .fetch_optional(pool)
    .await?;
    if migrated.is_some() {
        return Ok(());
    }
    let Ok(contents) = tokio::fs::read_to_string("/app/data/reaction_roles.json").await else {
        return Ok(());
    };
    let Ok(entries) = serde_json::from_str::<serde_json::Map<String, Value>>(&contents) else {
        tracing::warn!(
            "Could not parse legacy /app/data/reaction_roles.json; leaving it untouched"
        );
        return Ok(());
    };
    let mut transaction = pool.begin().await?;
    for (message_id, data) in entries {
        let Ok(message_id) = message_id.parse::<u64>() else {
            continue;
        };
        let Some(bindings) = data.as_object() else {
            continue;
        };
        let max_selections = bindings
            .get("_max_selections")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(25);
        let panel_name = bindings.get("_panel").and_then(Value::as_str);
        sqlx::query("INSERT OR IGNORE INTO reaction_role_panels (message_id,max_selections,panel_name) VALUES (?,?,?)")
            .bind(message_id as i64).bind(max_selections as i64).bind(panel_name).execute(&mut *transaction).await?;
        for (emoji, role) in bindings {
            if emoji.starts_with('_') {
                continue;
            }
            let Some(role_id) = role.as_u64() else {
                continue;
            };
            sqlx::query("INSERT OR IGNORE INTO reaction_role_bindings (message_id,emoji,role_id) VALUES (?,?,?)")
                .bind(message_id as i64).bind(emoji).bind(role_id as i64).execute(&mut *transaction).await?;
        }
    }
    sqlx::query("INSERT INTO reaction_role_migrations (name) VALUES ('reaction_roles_json')")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn create_panel(
    pool: &sqlx::SqlitePool,
    message_id: u64,
    guild_id: u64,
    channel_id: u64,
    max_selections: u8,
    bindings: &[(String, u64)],
) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("INSERT OR REPLACE INTO reaction_role_panels (message_id,guild_id,channel_id,max_selections,panel_name) VALUES (?,?,?,?,NULL)")
        .bind(message_id as i64).bind(guild_id as i64).bind(channel_id as i64).bind(max_selections as i64).execute(&mut *transaction).await?;
    sqlx::query("DELETE FROM reaction_role_bindings WHERE message_id=?")
        .bind(message_id as i64)
        .execute(&mut *transaction)
        .await?;
    for (emoji, role_id) in bindings {
        sqlx::query("INSERT INTO reaction_role_bindings (message_id,emoji,role_id) VALUES (?,?,?)")
            .bind(message_id as i64)
            .bind(emoji)
            .bind(*role_id as i64)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn panel(
    pool: &sqlx::SqlitePool,
    message_id: u64,
) -> Result<Option<PanelBinding>, sqlx::Error> {
    let row = sqlx::query("SELECT max_selections FROM reaction_role_panels WHERE message_id=?")
        .bind(message_id as i64)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| PanelBinding {
        max_selections: sqlx::Row::try_get::<i64, _>(&row, "max_selections")
            .unwrap_or_default()
            .clamp(0, 25) as u8,
    }))
}

pub async fn role_for_emoji(
    pool: &sqlx::SqlitePool,
    message_id: u64,
    emoji: &str,
) -> Result<Option<u64>, sqlx::Error> {
    let role_id: Option<i64> = sqlx::query_scalar(
        "SELECT role_id FROM reaction_role_bindings WHERE message_id=? AND emoji=?",
    )
    .bind(message_id as i64)
    .bind(emoji)
    .fetch_optional(pool)
    .await?;
    Ok(role_id.map(|id| id as u64))
}

pub async fn roles_for_message(
    pool: &sqlx::SqlitePool,
    message_id: u64,
) -> Result<Vec<u64>, sqlx::Error> {
    let rows = sqlx::query_scalar::<_, i64>(
        "SELECT role_id FROM reaction_role_bindings WHERE message_id=?",
    )
    .bind(message_id as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|id| id as u64).collect())
}

pub async fn remove_panel(pool: &sqlx::SqlitePool, message_id: u64) -> Result<(), sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM reaction_role_bindings WHERE message_id=?")
        .bind(message_id as i64)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("DELETE FROM reaction_role_panels WHERE message_id=?")
        .bind(message_id as i64)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}
