use super::{BuilderDraft, BuilderField};
use serde_json::Value;

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("CREATE TABLE IF NOT EXISTS builder_templates (name TEXT PRIMARY KEY, state_json TEXT NOT NULL, updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .execute(pool).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS builder_migrations (name TEXT PRIMARY KEY, completed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)")
        .execute(pool).await?;
    migrate_json_templates(pool).await?;
    Ok(())
}

async fn migrate_json_templates(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let migrated: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM builder_migrations WHERE name='embed_templates_json'")
            .fetch_optional(pool)
            .await?;
    if migrated.is_some() {
        return Ok(());
    }
    let Ok(contents) = tokio::fs::read_to_string("/app/data/embed_templates.json").await else {
        return Ok(());
    };
    let Ok(templates) = serde_json::from_str::<serde_json::Map<String, Value>>(&contents) else {
        tracing::warn!(
            "Could not parse legacy /app/data/embed_templates.json; leaving it untouched"
        );
        return Ok(());
    };
    let mut transaction = pool.begin().await?;
    for (name, value) in templates {
        let mut draft = BuilderDraft {
            channel_id: 0,
            target_message_id: None,
            title: String::new(),
            description: String::new(),
            content: String::new(),
            color: 0x5865F2,
            thumbnail_url: String::new(),
            image_url: String::new(),
            author_name: String::new(),
            footer_text: String::new(),
            fields: Vec::new(),
            allow_pings: false,
        };
        if let Some(state) = value.get("v2_state") {
            draft.title = string(state, "title");
            draft.description = string(state, "description");
            draft.color = state
                .get("color")
                .and_then(Value::as_u64)
                .unwrap_or(0x5865F2) as u32;
            draft.thumbnail_url = string(state, "top_image_url");
            draft.image_url = string(state, "bottom_image_url");
            draft.author_name = string(state, "author_name");
            draft.footer_text = string(state, "footer_text");
            if let Some(fields) = state.get("fields").and_then(Value::as_array) {
                draft.fields = fields
                    .iter()
                    .filter_map(|field| {
                        Some(BuilderField {
                            name: field.get("name")?.as_str()?.to_owned(),
                            value: field.get("value")?.as_str()?.to_owned(),
                        })
                    })
                    .collect();
            }
        } else if let Some(embed) = value.get("embed") {
            draft.title = string(embed, "title");
            draft.description = string(embed, "description");
            draft.color = embed
                .get("color")
                .and_then(Value::as_u64)
                .unwrap_or(0x5865F2) as u32;
            draft.thumbnail_url = embed
                .get("thumbnail")
                .and_then(|v| v.get("url"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| string(&value, "before_image_url"));
            draft.image_url = embed
                .get("image")
                .and_then(|v| v.get("url"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default();
            draft.author_name = embed
                .get("author")
                .and_then(|v| v.get("name"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default();
            draft.footer_text = embed
                .get("footer")
                .and_then(|v| v.get("text"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_default();
            if let Some(fields) = embed.get("fields").and_then(Value::as_array) {
                draft.fields = fields
                    .iter()
                    .filter_map(|field| {
                        Some(BuilderField {
                            name: field.get("name")?.as_str()?.to_owned(),
                            value: field.get("value")?.as_str()?.to_owned(),
                        })
                    })
                    .collect();
            }
        }
        draft.content = string(&value, "content");
        let state = serde_json::to_string(&draft).unwrap_or_default();
        sqlx::query("INSERT OR IGNORE INTO builder_templates (name,state_json) VALUES (?,?)")
            .bind(name)
            .bind(state)
            .execute(&mut *transaction)
            .await?;
    }
    sqlx::query("INSERT INTO builder_migrations (name) VALUES ('embed_templates_json')")
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

pub async fn save_template(
    pool: &sqlx::SqlitePool,
    name: &str,
    draft: &BuilderDraft,
) -> Result<(), sqlx::Error> {
    let mut template = draft.clone();
    template.channel_id = 0;
    template.target_message_id = None;
    template.allow_pings = false;
    let state = serde_json::to_string(&template)
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    sqlx::query("INSERT INTO builder_templates (name,state_json) VALUES (?,?) ON CONFLICT(name) DO UPDATE SET state_json=excluded.state_json,updated_at=CURRENT_TIMESTAMP")
        .bind(name).bind(state).execute(pool).await?;
    Ok(())
}

pub async fn list_templates(pool: &sqlx::SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT name FROM builder_templates ORDER BY name COLLATE NOCASE LIMIT 25")
        .fetch_all(pool)
        .await
}

pub async fn get_template(
    pool: &sqlx::SqlitePool,
    name: &str,
) -> Result<Option<BuilderDraft>, sqlx::Error> {
    let state: Option<String> =
        sqlx::query_scalar("SELECT state_json FROM builder_templates WHERE name=?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
    Ok(state.and_then(|json| serde_json::from_str(&json).ok()))
}
