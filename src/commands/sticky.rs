use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::Mutex;

// -----------------------------------------------------------------------------
// Modals
// -----------------------------------------------------------------------------

#[derive(poise::Modal)]
#[name = "Sticky Text Message"]
struct StickyTextModal {
    #[name = "Message Content"]
    #[placeholder = "Type the message you want to keep at the bottom..."]
    #[paragraph]
    message_content: String,
}

#[derive(poise::Modal)]
#[name = "Sticky Embed Message"]
struct StickyEmbedModal {
    #[name = "Title"]
    #[placeholder = "e.g. 📢 Important Announcement"]
    embed_title: Option<String>,

    #[name = "Description"]
    #[placeholder = "e.g. Please check out the links below..."]
    #[paragraph]
    embed_desc: String,

    #[name = "Color (Hex Code)"]
    #[placeholder = "e.g. #ff0000 or 5865F2"]
    embed_color: Option<String>,

    #[name = "Thumbnail URL"]
    #[placeholder = "e.g. https://example.com/image.png"]
    embed_thumb: Option<String>,

    #[name = "Footer Text"]
    #[placeholder = "e.g. Updated daily"]
    embed_footer: Option<String>,
}

// -----------------------------------------------------------------------------
// Database model
// -----------------------------------------------------------------------------

#[derive(sqlx::FromRow)]
pub struct StickyConfig {
    pub guild_id: i64,
    pub message_id: Option<i64>,
    pub content: Option<String>,
    pub embed_data: Option<String>,
}

// -----------------------------------------------------------------------------
// Per-channel locking
// -----------------------------------------------------------------------------

/// Mutexes intentionally remain in the map after a sticky is removed.
/// Removing a mutex while another task holds it could allow a second mutex
/// to be created for the same channel, defeating synchronization.
async fn get_channel_lock(data: &crate::Data, channel_id: u64) -> Arc<Mutex<()>> {
    let mut locks = data.sticky_locks.write().await;

    locks
        .entry(channel_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

// -----------------------------------------------------------------------------
// Database operations
// -----------------------------------------------------------------------------

/// Creates or migrates the sticky table to use a composite guild/channel key.
///
/// The migration preserves existing configurations and is safe to run at
/// every startup.
pub async fn init_db(pool: &SqlitePool) -> Result<(), Error> {
    let mut tx = pool.begin().await?;

    // Ensure the original table exists if this is a fresh installation.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sticky_posts (
            guild_id INTEGER NOT NULL,
            channel_id INTEGER NOT NULL,
            message_id INTEGER,
            content TEXT,
            embed_data TEXT,
            PRIMARY KEY (guild_id, channel_id)
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    // Build the replacement table.
    sqlx::query("DROP TABLE IF EXISTS sticky_posts_new")
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE sticky_posts_new (
            guild_id INTEGER NOT NULL,
            channel_id INTEGER NOT NULL,
            message_id INTEGER,
            content TEXT,
            embed_data TEXT,
            PRIMARY KEY (guild_id, channel_id)
        )
        "#,
    )
    .execute(&mut *tx)
    .await?;

    // Copy existing configurations into the new table.
    sqlx::query(
        r#"
        INSERT OR REPLACE INTO sticky_posts_new (
            guild_id,
            channel_id,
            message_id,
            content,
            embed_data
        )
        SELECT
            guild_id,
            channel_id,
            message_id,
            content,
            embed_data
        FROM sticky_posts
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query("DROP TABLE sticky_posts")
        .execute(&mut *tx)
        .await?;

    sqlx::query("ALTER TABLE sticky_posts_new RENAME TO sticky_posts")
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(())
}

pub async fn load_sticky_channels(
    pool: &SqlitePool,
) -> Result<std::collections::HashSet<u64>, Error> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT channel_id FROM sticky_posts")
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(|(id,)| u64::try_from(id).ok())
        .collect())
}

pub async fn save_sticky_config(
    pool: &SqlitePool,
    guild_id: u64,
    channel_id: u64,
    content: Option<&str>,
    embed_data: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        r#"
        INSERT INTO sticky_posts (
            guild_id,
            channel_id,
            message_id,
            content,
            embed_data
        )
        VALUES (?, ?, NULL, ?, ?)
        ON CONFLICT(guild_id, channel_id) DO UPDATE SET
            content = excluded.content,
            embed_data = excluded.embed_data
        "#,
    )
    .bind(guild_id as i64)
    .bind(channel_id as i64)
    .bind(content)
    .bind(embed_data)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn update_sticky_message_id(
    pool: &SqlitePool,
    guild_id: i64,
    channel_id: u64,
    message_id: u64,
) -> Result<(), Error> {
    sqlx::query(
        r#"
        UPDATE sticky_posts
        SET message_id = ?
        WHERE guild_id = ? AND channel_id = ?
        "#,
    )
    .bind(message_id as i64)
    .bind(guild_id)
    .bind(channel_id as i64)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_sticky_config(
    pool: &SqlitePool,
    guild_id: i64,
    channel_id: u64,
) -> Result<Option<StickyConfig>, Error> {
    let config = sqlx::query_as::<_, StickyConfig>(
        r#"
        SELECT guild_id, message_id, content, embed_data
        FROM sticky_posts
        WHERE guild_id = ? AND channel_id = ?
        "#,
    )
    .bind(guild_id)
    .bind(channel_id as i64)
    .fetch_optional(pool)
    .await?;

    Ok(config)
}

pub async fn delete_sticky_config(
    pool: &SqlitePool,
    guild_id: i64,
    channel_id: u64,
) -> Result<(), Error> {
    sqlx::query("DELETE FROM sticky_posts WHERE guild_id = ? AND channel_id = ?")
        .bind(guild_id)
        .bind(channel_id as i64)
        .execute(pool)
        .await?;

    Ok(())
}

/// Encodes the sticky identity using zero-width characters so a sent message
/// can be recovered if Discord accepts it but the database write fails.
fn sticky_marker(guild_id: i64, channel_id: u64) -> String {
    let mut marker = String::from("\u{2063}");
    let symbols = ['\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}'];
    for byte in guild_id
        .to_be_bytes()
        .into_iter()
        .chain(channel_id.to_be_bytes())
    {
        for shift in [6, 4, 2, 0] {
            marker.push(symbols[((byte >> shift) & 0b11) as usize]);
        }
    }
    marker.push('\u{2063}');
    marker
}

async fn find_sticky_messages(
    ctx: &serenity::Context,
    channel_id: serenity::ChannelId,
    marker: &str,
) -> Result<Vec<serenity::Message>, Error> {
    let bot_id = ctx.cache.current_user().id;
    let messages = channel_id
        .messages(&ctx.http, serenity::GetMessages::new().limit(100))
        .await?;

    Ok(messages
        .into_iter()
        .filter(|message| message.author.id == bot_id && message.content.contains(marker))
        .collect())
}

async fn adopt_sticky_messages(
    ctx: &serenity::Context,
    data: &crate::Data,
    channel_id: serenity::ChannelId,
    config: &StickyConfig,
    marker: &str,
) -> Result<bool, Error> {
    let mut matches = find_sticky_messages(ctx, channel_id, marker)
        .await?
        .into_iter();
    let Some(message) = matches.next() else {
        return Ok(false);
    };

    update_sticky_message_id(
        &data.db,
        config.guild_id,
        channel_id.get(),
        message.id.get(),
    )
    .await?;

    for duplicate in matches {
        if let Err(error) = channel_id.delete_message(&ctx.http, duplicate.id).await {
            eprintln!(
                "[StickyPosts] Could not remove duplicate sticky {} in channel {channel_id}: {error}",
                duplicate.id
            );
        }
    }

    Ok(true)
}

// -----------------------------------------------------------------------------
// Embed reconstruction
// -----------------------------------------------------------------------------

fn build_embed(json_str: &str) -> Result<serenity::CreateEmbed, Error> {
    let value: serde_json::Value = serde_json::from_str(json_str)?;

    let mut builder = serenity::CreateEmbed::default();

    if let Some(title) = value.get("title").and_then(|v| v.as_str()) {
        builder = builder.title(title);
    }

    if let Some(description) = value.get("description").and_then(|v| v.as_str()) {
        builder = builder.description(description);
    }

    if let Some(url) = value.get("url").and_then(|v| v.as_str()) {
        builder = builder.url(url);
    }

    if let Some(color) = value.get("color").and_then(|v| v.as_u64()) {
        builder = builder.color(color as u32);
    }

    if let Some(thumbnail) = value
        .get("thumbnail")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
    {
        builder = builder.thumbnail(thumbnail);
    }

    if let Some(image) = value
        .get("image")
        .and_then(|v| v.get("url"))
        .and_then(|v| v.as_str())
    {
        builder = builder.image(image);
    }

    if let Some(author) = value.get("author") {
        if let Some(name) = author.get("name").and_then(|v| v.as_str()) {
            let mut author_builder = serenity::CreateEmbedAuthor::new(name);

            if let Some(url) = author.get("url").and_then(|v| v.as_str()) {
                author_builder = author_builder.url(url);
            }

            if let Some(icon_url) = author.get("icon_url").and_then(|v| v.as_str()) {
                author_builder = author_builder.icon_url(icon_url);
            }

            builder = builder.author(author_builder);
        }
    }

    if let Some(footer) = value.get("footer") {
        if let Some(text) = footer.get("text").and_then(|v| v.as_str()) {
            let mut footer_builder = serenity::CreateEmbedFooter::new(text);

            if let Some(icon_url) = footer.get("icon_url").and_then(|v| v.as_str()) {
                footer_builder = footer_builder.icon_url(icon_url);
            }

            builder = builder.footer(footer_builder);
        }
    }

    if let Some(fields) = value.get("fields").and_then(|v| v.as_array()) {
        for field in fields {
            let name = field.get("name").and_then(|v| v.as_str());
            let field_value = field.get("value").and_then(|v| v.as_str());

            if let (Some(name), Some(field_value)) = (name, field_value) {
                let inline = field
                    .get("inline")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                builder = builder.field(name, field_value, inline);
            }
        }
    }

    Ok(builder)
}

// -----------------------------------------------------------------------------
// Sticky message operations
// -----------------------------------------------------------------------------

/// Acquires the channel lock and performs a sticky repost.
pub async fn stick_message(
    ctx: &serenity::Context,
    data: &crate::Data,
    guild_id: i64,
    channel_id: serenity::ChannelId,
) -> Result<(), Error> {
    let channel_lock = get_channel_lock(data, channel_id.get()).await;
    let _guard = channel_lock.lock().await;

    stick_message_locked(ctx, data, guild_id, channel_id).await
}

/// Performs the actual sticky repost.
///
/// The caller MUST already hold the channel's mutex.
async fn stick_message_locked(
    ctx: &serenity::Context,
    data: &crate::Data,
    guild_id: i64,
    channel_id: serenity::ChannelId,
) -> Result<(), Error> {
    let config = match get_sticky_config(&data.db, guild_id, channel_id.get()).await? {
        Some(config) => config,
        None => {
            data.sticky_channels.write().await.remove(&channel_id.get());
            return Ok(());
        }
    };

    let marker = sticky_marker(config.guild_id, channel_id.get());

    // If the last send reached Discord but its ID was not saved, reuse it
    // instead of posting another copy.
    if config.message_id.is_none()
        && adopt_sticky_messages(ctx, data, channel_id, &config, &marker).await?
    {
        return Ok(());
    }

    // Remove the previous sticky message before posting its replacement.
    if let Some(message_id) = config.message_id {
        let old_message_id = serenity::MessageId::new(message_id as u64);

        match channel_id.delete_message(&ctx.http, old_message_id).await {
            Ok(()) => {}
            Err(error)
                if matches!(
                    &error,
                    serenity::Error::Http(http_error)
                        if http_error.status_code() == Some(serenity::http::StatusCode::NOT_FOUND)
                ) => {}
            Err(error) => return Err(Box::new(error)),
        }

        sqlx::query(
            "UPDATE sticky_posts SET message_id = NULL WHERE guild_id = ? AND channel_id = ?",
        )
        .bind(config.guild_id)
        .bind(channel_id.get() as i64)
        .execute(&data.db)
        .await?;
    }

    let embed = match config.embed_data.as_deref() {
        Some(json_str) => Some(build_embed(json_str)?),
        None => None,
    };

    let mut builder = serenity::CreateMessage::new();

    if let Some(content) = config.content.as_deref() {
        builder = builder.content(format!("{content}{marker}"));
    } else {
        // Keep the identity invisible while still searchable after ambiguous
        // send failures or a failed database update.
        builder = builder.content(marker.clone());
    }

    if let Some(embed) = embed {
        builder = builder.embed(embed);
    }

    match channel_id.send_message(&ctx.http, builder).await {
        Ok(new_message) => {
            update_sticky_message_id(
                &data.db,
                config.guild_id,
                channel_id.get(),
                new_message.id.get(),
            )
            .await?;
        }
        Err(error) => {
            // Discord may have accepted the request before the connection
            // failed. Search for our identity marker before allowing a retry.
            if adopt_sticky_messages(ctx, data, channel_id, &config, &marker).await? {
                return Ok(());
            }

            eprintln!("[StickyPosts] Error sending sticky in channel {channel_id}: {error}");
            return Err(Box::new(error));
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Input validation
// -----------------------------------------------------------------------------

fn parse_embed_color(input: Option<&str>) -> Result<u32, &'static str> {
    let Some(input) = input else {
        return Ok(0x5865F2);
    };

    let clean = input.trim().trim_start_matches('#');

    if clean.is_empty() {
        return Ok(0x5865F2);
    }

    if clean.len() != 6 || !clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(
            "Invalid color. Please enter exactly six hexadecimal digits, \
             optionally prefixed with # (for example, #5865F2).",
        );
    }

    u32::from_str_radix(clean, 16).map_err(|_| "Invalid hexadecimal color.")
}

// -----------------------------------------------------------------------------
// Slash commands
// -----------------------------------------------------------------------------

/// Create or replace a sticky text message in this channel.
#[poise::command(slash_command)]
pub async fn text(ctx: Context<'_>) -> Result<(), Error> {
    if let poise::Context::Application(app_ctx) = ctx {
        let modal_data = poise::execute_modal::<_, _, StickyTextModal>(app_ctx, None, None).await?;

        if let Some(modal_data) = modal_data {
            app_ctx.defer_ephemeral().await?;

            if modal_data.message_content.trim().is_empty() {
                app_ctx
                    .say("🛑 Sticky message content cannot be empty.")
                    .await?;

                return Ok(());
            }

            let guild_id = app_ctx.guild_id().ok_or("Must be in a guild.")?;
            let channel_id = app_ctx.channel_id();

            let channel_lock = get_channel_lock(app_ctx.data(), channel_id.get()).await;
            let _guard = channel_lock.lock().await;

            save_sticky_config(
                &app_ctx.data().db,
                guild_id.get(),
                channel_id.get(),
                Some(&modal_data.message_content),
                None,
            )
            .await?;

            app_ctx
                .data()
                .sticky_channels
                .write()
                .await
                .insert(channel_id.get());

            stick_message_locked(
                app_ctx.serenity_context(),
                app_ctx.data(),
                guild_id.get() as i64,
                channel_id,
            )
            .await?;

            app_ctx
                .send(
                    poise::CreateReply::default()
                        .content("✅ Sticky text message set for this channel!")
                        .ephemeral(true),
                )
                .await?;
        }
    }

    Ok(())
}

/// Create or replace a sticky embed in this channel.
#[poise::command(slash_command)]
pub async fn embed(ctx: Context<'_>) -> Result<(), Error> {
    if let poise::Context::Application(app_ctx) = ctx {
        let modal_data =
            poise::execute_modal::<_, _, StickyEmbedModal>(app_ctx, None, None).await?;

        if let Some(modal_data) = modal_data {
            app_ctx.defer_ephemeral().await?;

            if modal_data.embed_desc.trim().is_empty() {
                app_ctx.say("🛑 Embed description cannot be empty.").await?;
                return Ok(());
            }

            let color = match parse_embed_color(modal_data.embed_color.as_deref()) {
                Ok(color) => color,
                Err(error) => {
                    app_ctx.say(format!("🛑 {error}")).await?;
                    return Ok(());
                }
            };

            let thumbnail = modal_data.embed_thumb.filter(|s| !s.trim().is_empty());

            if let Some(url) = thumbnail.as_deref() {
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    app_ctx
                        .say("🛑 Thumbnail URL must start with http:// or https://.")
                        .await?;
                    return Ok(());
                }
            }

            let guild_id = app_ctx.guild_id().ok_or("Must be in a guild.")?;
            let channel_id = app_ctx.channel_id();

            let embed_dict = serde_json::json!({
                "title": modal_data.embed_title
                    .filter(|s| !s.trim().is_empty()),
                "description": modal_data.embed_desc,
                "color": color,
                "thumbnail": thumbnail
                    .map(|url| serde_json::json!({ "url": url })),
                "footer": modal_data.embed_footer
                    .filter(|s| !s.trim().is_empty())
                    .map(|txt| serde_json::json!({ "text": txt })),
            });

            let json_str = serde_json::to_string(&embed_dict)?;

            let channel_lock = get_channel_lock(app_ctx.data(), channel_id.get()).await;
            let _guard = channel_lock.lock().await;

            save_sticky_config(
                &app_ctx.data().db,
                guild_id.get(),
                channel_id.get(),
                None,
                Some(&json_str),
            )
            .await?;

            app_ctx
                .data()
                .sticky_channels
                .write()
                .await
                .insert(channel_id.get());

            stick_message_locked(
                app_ctx.serenity_context(),
                app_ctx.data(),
                guild_id.get() as i64,
                channel_id,
            )
            .await?;

            app_ctx
                .send(
                    poise::CreateReply::default()
                        .content("✅ Sticky embed message set for this channel!")
                        .ephemeral(true),
                )
                .await?;
        }
    }

    Ok(())
}

/// Remove the sticky message configured for this channel.
#[poise::command(slash_command)]
pub async fn remove(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx.guild_id().ok_or("Must be in a guild.")?.get() as i64;
    let channel_id = ctx.channel_id();

    let channel_lock = get_channel_lock(ctx.data(), channel_id.get()).await;
    let _guard = channel_lock.lock().await;

    let Some(config) = get_sticky_config(&ctx.data().db, guild_id, channel_id.get()).await? else {
        ctx.data()
            .sticky_channels
            .write()
            .await
            .remove(&channel_id.get());

        ctx.say("Nothing to remove here.").await?;
        return Ok(());
    };

    if let Some(message_id) = config.message_id {
        if let Err(error) = channel_id
            .delete_message(
                &ctx.serenity_context().http,
                serenity::MessageId::new(message_id as u64),
            )
            .await
        {
            eprintln!(
                "[StickyPosts] Could not delete sticky message \
                 {message_id} during removal: {error}"
            );
        }
    }

    delete_sticky_config(&ctx.data().db, guild_id, channel_id.get()).await?;

    ctx.data()
        .sticky_channels
        .write()
        .await
        .remove(&channel_id.get());

    // Keep the channel mutex in sticky_locks.

    ctx.say("Removed sticky message from this channel.").await?;

    Ok(())
}

/// Remove all sticky messages configured in this server.
#[poise::command(slash_command, rename = "remove-all")]
pub async fn remove_all(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx.guild_id().ok_or("Must be in a guild.")?;
    let guild_id_db = guild_id.get() as i64;

    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT channel_id FROM sticky_posts WHERE guild_id = ?")
            .bind(guild_id_db)
            .fetch_all(&ctx.data().db)
            .await?;

    if rows.is_empty() {
        ctx.say("ℹ️ No active sticky messages found in this server.")
            .await?;

        return Ok(());
    }

    let mut removed_count = 0;

    for (channel_id_raw,) in rows {
        let Ok(channel_id) = u64::try_from(channel_id_raw) else {
            continue;
        };

        let channel = serenity::ChannelId::new(channel_id);

        let channel_lock = get_channel_lock(ctx.data(), channel_id).await;
        let _guard = channel_lock.lock().await;

        let Some(config) = get_sticky_config(&ctx.data().db, guild_id_db, channel_id).await? else {
            continue;
        };

        if let Some(message_id) = config.message_id {
            if let Err(error) = channel
                .delete_message(
                    &ctx.serenity_context().http,
                    serenity::MessageId::new(message_id as u64),
                )
                .await
            {
                eprintln!(
                    "[StickyPosts] Could not delete sticky message \
                     {message_id} during remove-all: {error}"
                );
            }
        }

        delete_sticky_config(&ctx.data().db, guild_id_db, channel_id).await?;

        ctx.data().sticky_channels.write().await.remove(&channel_id);

        // Keep the channel mutex in sticky_locks.
        removed_count += 1;
    }

    ctx.say(format!(
        "Removed all {removed_count} sticky messages across the server."
    ))
    .await?;

    Ok(())
}

// -----------------------------------------------------------------------------
// Command groups
// -----------------------------------------------------------------------------

/// Create, update, or remove sticky messages in server channels.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_MESSAGES",
    subcommands("text", "embed", "remove", "remove_all")
)]
pub async fn sticky(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Pin a message as the sticky message for its channel.
#[poise::command(
    context_menu_command = "Pin as Sticky",
    guild_only,
    default_member_permissions = "MANAGE_MESSAGES"
)]
pub async fn pin_as_sticky(
    ctx: Context<'_>,
    #[description = "Message to stick"] message: serenity::Message,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    if message.content.trim().is_empty() && message.embeds.is_empty() {
        ctx.say("🛑 This message has no text content or embeds to sticky-pin.")
            .await?;

        return Ok(());
    }

    let guild_id = ctx.guild_id().ok_or("Must be in a guild.")?;
    let channel_id = ctx.channel_id();

    let content = if message.content.is_empty() {
        None
    } else {
        Some(message.content.as_str())
    };

    let embed_data = if let Some(first_embed) = message.embeds.first() {
        Some(serde_json::to_string(first_embed)?)
    } else {
        None
    };

    let channel_lock = get_channel_lock(ctx.data(), channel_id.get()).await;
    let _guard = channel_lock.lock().await;

    save_sticky_config(
        &ctx.data().db,
        guild_id.get(),
        channel_id.get(),
        content,
        embed_data.as_deref(),
    )
    .await?;

    ctx.data()
        .sticky_channels
        .write()
        .await
        .insert(channel_id.get());

    stick_message_locked(
        ctx.serenity_context(),
        ctx.data(),
        guild_id.get() as i64,
        channel_id,
    )
    .await?;

    ctx.say("✅ Message pinned as a sticky to the bottom of this channel!")
        .await?;

    Ok(())
}
