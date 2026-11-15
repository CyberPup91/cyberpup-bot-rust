//! Anonymous vents with private author records, cooldowns, and vents-only bans.
use crate::{Context, Data, Error};
pub mod management;
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, InputTextStyle,
};
use serenity::Mentionable;
use sqlx::Row;

/// Configure and publish the anonymous vents panel.
#[poise::command(
    slash_command,
    subcommands("manage", "configure", "publish", "ban", "unban")
)]
pub async fn vents(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Open the Vents settings dashboard.
#[poise::command(slash_command, required_permissions = "MANAGE_GUILD")]
pub async fn manage(ctx: Context<'_>) -> Result<(), Error> {
    if !is_trusted_command(ctx).await {
        ctx.send(
            poise::CreateReply::default()
                .content("🛑 Unauthorized.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    management::manage(ctx).await
}

#[allow(deprecated)]
async fn is_trusted_command(ctx: Context<'_>) -> bool {
    let user = ctx.author().id.get();
    if env_ids("TRUSTED_USER_IDS").contains(&user) {
        return true;
    }
    let Some(member) = ctx.author_member().await else {
        return false;
    };
    if member
        .roles
        .iter()
        .any(|role| env_ids("ALLOWED_ROLE_IDS").contains(&role.get()))
    {
        return true;
    }
    member
        .permissions(&ctx.serenity_context().cache)
        .is_ok_and(|permissions| permissions.administrator())
}

fn env_ids(key: &str) -> std::collections::HashSet<u64> {
    std::env::var(key)
        .unwrap_or_default()
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect()
}

fn trusted_interaction(i: &serenity::all::ComponentInteraction) -> bool {
    if env_ids("TRUSTED_USER_IDS").contains(&i.user.id.get()) {
        return true;
    }
    if i.member.as_ref().is_some_and(|member| {
        member
            .roles
            .iter()
            .any(|role| env_ids("ALLOWED_ROLE_IDS").contains(&role.get()))
    }) {
        return true;
    }
    i.member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(|permissions| permissions.administrator())
}

/// Set the vent and staff log channels and the per-user cooldown.
#[poise::command(slash_command, required_permissions = "MANAGE_GUILD")]
pub async fn configure(
    ctx: Context<'_>,
    vent_channel: serenity::ChannelId,
    log_channel: serenity::ChannelId,
    cooldown_seconds: i64,
) -> Result<(), Error> {
    if !is_trusted_command(ctx).await {
        ctx.send(
            poise::CreateReply::default()
                .content("🛑 Unauthorized.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let Some(guild) = ctx.guild_id() else {
        ctx.say("Use this command in a server.").await?;
        return Ok(());
    };
    if cooldown_seconds < 0 {
        ctx.send(
            poise::CreateReply::default()
                .content("Cooldown must be zero or greater.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let previous =
        sqlx::query("SELECT vent_channel_id,panel_message_id FROM vent_config WHERE guild_id=?")
            .bind(guild.get() as i64)
            .fetch_optional(&ctx.data().vents_db)
            .await?;
    if let Some(previous) = previous {
        let old_channel = previous.try_get::<Option<i64>, _>("vent_channel_id")?;
        let old_panel = previous.try_get::<Option<i64>, _>("panel_message_id")?;
        if old_channel.is_some_and(|id| id as u64 != vent_channel.get()) {
            if let (Some(channel), Some(panel)) = (old_channel, old_panel) {
                let _ = serenity::ChannelId::new(channel as u64)
                    .delete_message(
                        ctx.serenity_context(),
                        serenity::MessageId::new(panel as u64),
                    )
                    .await;
            }
        }
    }
    sqlx::query("INSERT INTO vent_config(guild_id, vent_channel_id, log_channel_id, cooldown_seconds) VALUES(?,?,?,?) ON CONFLICT(guild_id) DO UPDATE SET panel_message_id=CASE WHEN vent_config.vent_channel_id<>excluded.vent_channel_id THEN NULL ELSE vent_config.panel_message_id END,vent_channel_id=excluded.vent_channel_id, log_channel_id=excluded.log_channel_id, cooldown_seconds=excluded.cooldown_seconds")
        .bind(guild.get() as i64).bind(vent_channel.get() as i64).bind(log_channel.get() as i64).bind(cooldown_seconds).execute(&ctx.data().vents_db).await?;
    ctx.send(
        poise::CreateReply::default()
            .content("✅ Vents channels and cooldown saved.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Publish the anonymous submission panel in the configured vent channel.
#[poise::command(slash_command, required_permissions = "MANAGE_GUILD")]
pub async fn publish(ctx: Context<'_>) -> Result<(), Error> {
    if !is_trusted_command(ctx).await {
        ctx.send(
            poise::CreateReply::default()
                .content("🛑 Unauthorized.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let Some(guild) = ctx.guild_id() else {
        ctx.say("Use this command in a server.").await?;
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let row = sqlx::query("SELECT vent_channel_id FROM vent_config WHERE guild_id=?")
        .bind(guild.get() as i64)
        .fetch_optional(&ctx.data().vents_db)
        .await?;
    let Some(row) = row else {
        ctx.say("Configure vents first with `/vents configure`.")
            .await?;
        return Ok(());
    };
    let channel = row
        .try_get::<Option<i64>, _>("vent_channel_id")?
        .unwrap_or_default() as u64;
    if channel == 0 {
        ctx.say("Configure the vent channel before publishing the panel.")
            .await?;
        return Ok(());
    }
    stick_panel(
        ctx.serenity_context(),
        ctx.data(),
        guild,
        serenity::ChannelId::new(channel),
        true,
    )
    .await?;
    ctx.send(
        poise::CreateReply::default()
            .content("✅ Vents panel published.")
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Ban a user from submitting vents (this does not ban them from the server).
#[poise::command(slash_command, required_permissions = "MANAGE_GUILD")]
pub async fn ban(
    ctx: Context<'_>,
    user: serenity::User,
    #[rest] reason: Option<String>,
) -> Result<(), Error> {
    if !is_trusted_command(ctx).await {
        ctx.send(
            poise::CreateReply::default()
                .content("🛑 Unauthorized.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let Some(guild) = ctx.guild_id() else {
        ctx.say("Use this command in a server.").await?;
        return Ok(());
    };
    let already_banned: i64 =
        sqlx::query("SELECT EXISTS(SELECT 1 FROM vent_bans WHERE guild_id=? AND user_id=?)")
            .bind(guild.get() as i64)
            .bind(user.id.get() as i64)
            .fetch_one(&ctx.data().vents_db)
            .await?
            .try_get(0)?;
    if already_banned != 0 {
        ctx.send(
            poise::CreateReply::default()
                .content(format!(
                    "ℹ️ {} is already banned from vents.",
                    user.mention()
                ))
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let reason = reason.unwrap_or_else(|| "Manual ban".to_owned());
    sqlx::query("INSERT INTO vent_bans(guild_id,user_id,banned_by,reason,banned_at) VALUES(?,?,?,?,unixepoch()) ON CONFLICT(guild_id,user_id) DO UPDATE SET banned_by=excluded.banned_by,reason=excluded.reason,banned_at=excluded.banned_at")
        .bind(guild.get() as i64).bind(user.id.get() as i64).bind(ctx.author().id.get() as i64).bind(&reason).execute(&ctx.data().vents_db).await?;
    ctx.send(
        poise::CreateReply::default()
            .content(format!(
                "✅ {} has been banned from the vents system.\nReason: {}",
                user.mention(),
                reason
            ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Remove a vents-only ban.
#[poise::command(slash_command, required_permissions = "MANAGE_GUILD")]
pub async fn unban(ctx: Context<'_>, user: serenity::User) -> Result<(), Error> {
    if !is_trusted_command(ctx).await {
        ctx.send(
            poise::CreateReply::default()
                .content("🛑 Unauthorized.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let Some(guild) = ctx.guild_id() else {
        ctx.say("Use this command in a server.").await?;
        return Ok(());
    };
    let result = sqlx::query("DELETE FROM vent_bans WHERE guild_id=? AND user_id=?")
        .bind(guild.get() as i64)
        .bind(user.id.get() as i64)
        .execute(&ctx.data().vents_db)
        .await?;
    ctx.send(
        poise::CreateReply::default()
            .content(if result.rows_affected() > 0 {
                format!(
                    "✅ {} has been unbanned from the vents system.",
                    user.mention()
                )
            } else {
                format!("ℹ️ {} is not currently banned from vents.", user.mention())
            })
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

pub async fn init_db(db: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("CREATE TABLE IF NOT EXISTS vent_config(guild_id INTEGER PRIMARY KEY,vent_channel_id INTEGER,log_channel_id INTEGER,cooldown_seconds INTEGER NOT NULL DEFAULT 300,panel_message_id INTEGER,panel_title TEXT,panel_description TEXT,panel_footer TEXT)").execute(db).await?;
    for statement in [
        "ALTER TABLE vent_config ADD COLUMN cooldown_seconds INTEGER NOT NULL DEFAULT 300",
        "ALTER TABLE vent_config ADD COLUMN panel_message_id INTEGER",
        "ALTER TABLE vent_config ADD COLUMN panel_title TEXT",
        "ALTER TABLE vent_config ADD COLUMN panel_description TEXT",
        "ALTER TABLE vent_config ADD COLUMN panel_footer TEXT",
    ] {
        let _ = sqlx::query(statement).execute(db).await;
    }
    sqlx::query("CREATE TABLE IF NOT EXISTS vent_bans(guild_id INTEGER NOT NULL,user_id INTEGER NOT NULL,banned_by INTEGER,reason TEXT,banned_at INTEGER,PRIMARY KEY(guild_id,user_id))").execute(db).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS vent_cooldowns(guild_id INTEGER NOT NULL,user_id INTEGER NOT NULL,last_action_at TEXT,PRIMARY KEY(guild_id,user_id))").execute(db).await?;
    let _ = sqlx::query("ALTER TABLE vent_cooldowns ADD COLUMN last_action_at TEXT")
        .execute(db)
        .await;
    let _ = sqlx::query("UPDATE vent_cooldowns SET last_action_at=datetime(last_action,'unixepoch') WHERE last_action_at IS NULL").execute(db).await;
    sqlx::query("CREATE TABLE IF NOT EXISTS vents(id INTEGER PRIMARY KEY AUTOINCREMENT,guild_id INTEGER NOT NULL,message_id INTEGER UNIQUE,channel_id INTEGER NOT NULL,thread_id INTEGER,author_id INTEGER NOT NULL,content TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'active',created_at INTEGER NOT NULL)").execute(db).await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS vent_replies(id INTEGER PRIMARY KEY AUTOINCREMENT,vent_id INTEGER NOT NULL,message_id INTEGER UNIQUE,author_id INTEGER NOT NULL,content TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'active',created_at INTEGER NOT NULL)").execute(db).await?;
    let _ =
        sqlx::query("ALTER TABLE vent_replies ADD COLUMN status TEXT NOT NULL DEFAULT 'active'")
            .execute(db)
            .await;
    Ok(())
}

pub async fn handle_message(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
) -> Result<(), Error> {
    if message.author.bot {
        return Ok(());
    }
    let Some(guild) = message.guild_id else {
        return Ok(());
    };
    let config =
        sqlx::query("SELECT vent_channel_id,panel_message_id FROM vent_config WHERE guild_id=?")
            .bind(guild.get() as i64)
            .fetch_optional(&data.vents_db)
            .await?;
    let Some(config) = config else {
        return Ok(());
    };
    let channel_id = config
        .try_get::<Option<i64>, _>("vent_channel_id")?
        .unwrap_or_default() as u64;
    if message.channel_id.get() != channel_id
        || config.try_get::<Option<i64>, _>("panel_message_id")? == Some(message.id.get() as i64)
    {
        return Ok(());
    }
    stick_panel(
        ctx,
        data,
        guild,
        serenity::ChannelId::new(channel_id),
        false,
    )
    .await
}

async fn stick_panel(
    ctx: &serenity::Context,
    data: &Data,
    guild: serenity::GuildId,
    channel: serenity::ChannelId,
    _force: bool,
) -> Result<(), Error> {
    static LOCKS: std::sync::OnceLock<
        tokio::sync::Mutex<std::collections::HashMap<u64, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let lock = {
        let mut locks = locks.lock().await;
        locks
            .entry(channel.get())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().await;
    static LAST_STICK: std::sync::OnceLock<
        tokio::sync::Mutex<std::collections::HashMap<u64, tokio::time::Instant>>,
    > = std::sync::OnceLock::new();
    let last_stick =
        LAST_STICK.get_or_init(|| tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let remaining = {
        let times = last_stick.lock().await;
        times
            .get(&channel.get())
            .map(|last| std::time::Duration::from_secs(3).saturating_sub(last.elapsed()))
            .unwrap_or_default()
    };
    if !remaining.is_zero() {
        tokio::time::sleep(remaining).await;
    }
    let config = sqlx::query("SELECT panel_message_id,panel_title,panel_description,panel_footer FROM vent_config WHERE guild_id=?").bind(guild.get() as i64).fetch_optional(&data.vents_db).await?;
    let Some(config) = config else {
        return Ok(());
    };
    let old = config.try_get::<Option<i64>, _>("panel_message_id")?;
    if let Some(old_id) = old {
        let _ = channel
            .delete_message(ctx, serenity::MessageId::new(old_id as u64))
            .await;
    }
    let embed = CreateEmbed::new()
        .title(config.try_get::<Option<String>,_>("panel_title")?.unwrap_or_else(|| "📝 Anonymous Vents".into()))
        .description(config.try_get::<Option<String>,_>("panel_description")?.unwrap_or_else(|| "Need to get something off your chest?\n\nClick the button below to submit an **anonymous** vent. Your identity is never shown publicly — only staff can see it in the private logs for safety.\n\nReplies are also anonymous and appear in a thread under each vent.".into()))
        .footer(serenity::CreateEmbedFooter::new(config.try_get::<Option<String>,_>("panel_footer")?.unwrap_or_else(|| "Be kind. Abuse will result in a vents ban.".into())))
        .color(0x800080);
    let components = vec![CreateActionRow::Buttons(vec![CreateButton::new(format!(
        "vent_panel:submit:{}",
        guild.get()
    ))
    .label("Submit a vent")
    .emoji('📝')
    .style(ButtonStyle::Primary)])];
    let panel = channel
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .embed(embed)
                .components(components),
        )
        .await?;
    sqlx::query("UPDATE vent_config SET panel_message_id=? WHERE guild_id=?")
        .bind(panel.id.get() as i64)
        .bind(guild.get() as i64)
        .execute(&data.vents_db)
        .await?;
    last_stick
        .lock()
        .await
        .insert(channel.get(), tokio::time::Instant::now());
    Ok(())
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    i: &serenity::all::ComponentInteraction,
) -> Result<bool, Error> {
    if let Some(guild) = i.guild_id {
        if let Some(rest) = i
            .data
            .custom_id
            .strip_prefix("vent_log:")
            .or_else(|| i.data.custom_id.strip_prefix("vents:log:"))
        {
            let mut parts = rest.split(':');
            let action = parts.next().unwrap_or_default();
            let kind = parts.next().unwrap_or_default();
            let id = parts
                .next()
                .and_then(|x| x.parse::<i64>().ok())
                .unwrap_or_default();
            if !trusted_interaction(i) {
                respond(ctx, i, "Only trusted staff can use vent log actions.").await?;
                return Ok(true);
            }
            let table = match kind {
                "vent" => "vents",
                "reply" => "vent_replies",
                _ => {
                    respond(ctx, i, "Unknown log record.").await?;
                    return Ok(true);
                }
            };
            let record = sqlx::query(&format!("SELECT * FROM {table} WHERE id=?"))
                .bind(id)
                .fetch_optional(&data.vents_db)
                .await?;
            let Some(record) = record else {
                respond(ctx, i, "Log record not found.").await?;
                return Ok(true);
            };
            match action {
                "reveal" => {
                    let author: i64 = record.try_get("author_id")?;
                    let content: String = record
                        .try_get::<String, _>("content")?
                        .replace("@everyone", "@\u{200b}everyone")
                        .replace("@here", "@\u{200b}here")
                        .replace("<@", "<@\u{200b}");
                    let parent = if kind == "reply" {
                        format!(
                            "Parent vent record: `{}`\n",
                            record.try_get::<i64, _>("vent_id")?
                        )
                    } else {
                        String::new()
                    };
                    let public_message_id: i64 = record.try_get("message_id")?;
                    let content = content.chars().take(2000).collect::<String>();
                    let mut embed = CreateEmbed::new()
                        .title("👁️ Revealed Vent Log Details")
                        .color(0xE67E22)
                        .field("Author", format!("<@{author}> (`{author}`)"), false)
                        .field(
                            "Content",
                            content.chars().take(1024).collect::<String>(),
                            false,
                        );
                    if content.chars().count() > 1024 {
                        embed = embed.field(
                            "Content (continued)",
                            content.chars().skip(1024).take(1024).collect::<String>(),
                            false,
                        );
                    }
                    let embed = embed.field(
                        "Record",
                        format!(
                            "{kind} #{id}\nPublic message ID: `{public_message_id}`\n{parent}Status: `{}`",
                            record.try_get::<String, _>("status")?
                        ),
                        false,
                    );
                    i.create_response(
                        ctx,
                        CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .embed(embed)
                                .ephemeral(true)
                                .allowed_mentions(serenity::CreateAllowedMentions::new()),
                        ),
                    )
                    .await?;
                }
                "remove" => {
                    if record.try_get::<String, _>("status")? == "removed" {
                        respond(ctx, i, "This post was already removed.").await?;
                        return Ok(true);
                    }
                    if kind == "vent" {
                        let channel = record.try_get::<i64, _>("channel_id")? as u64;
                        let message = record.try_get::<i64, _>("message_id")? as u64;
                        let _ = serenity::ChannelId::new(channel)
                            .delete_message(ctx, serenity::MessageId::new(message))
                            .await;
                        if let Some(thread) = record.try_get::<Option<i64>, _>("thread_id")? {
                            let _ = serenity::ChannelId::new(thread as u64).delete(ctx).await;
                        }
                    } else {
                        let parent = record.try_get::<i64, _>("vent_id")?;
                        if let Some(row) = sqlx::query("SELECT thread_id FROM vents WHERE id=?")
                            .bind(parent)
                            .fetch_optional(&data.vents_db)
                            .await?
                        {
                            if let Some(thread) = row.try_get::<Option<i64>, _>("thread_id")? {
                                let _ = serenity::ChannelId::new(thread as u64)
                                    .delete_message(
                                        ctx,
                                        serenity::MessageId::new(
                                            record.try_get::<i64, _>("message_id")? as u64,
                                        ),
                                    )
                                    .await;
                            }
                        }
                    }
                    sqlx::query(&format!("UPDATE {table} SET status='removed' WHERE id=?"))
                        .bind(id)
                        .execute(&data.vents_db)
                        .await?;
                    respond(
                        ctx,
                        i,
                        &format!("✅ Removed public {kind} #{id}. The author was not banned."),
                    )
                    .await?;
                }
                "ban" => {
                    let author = record.try_get::<i64, _>("author_id")?;
                    sqlx::query("INSERT INTO vent_bans(guild_id,user_id,banned_by,reason,banned_at) VALUES(?,?,?,?,unixepoch()) ON CONFLICT(guild_id,user_id) DO UPDATE SET banned_by=excluded.banned_by,reason=excluded.reason,banned_at=excluded.banned_at").bind(guild.get() as i64).bind(author).bind(i.user.id.get() as i64).bind(format!("Banned via log action ({kind} #{id})")).execute(&data.vents_db).await?;
                    respond(ctx,i,&format!("✅ User `{author}` is banned from vents. The public post remains; use Remove Post to delete it.")).await?;
                }
                _ => {
                    respond(ctx, i, "Unknown vent log action.").await?;
                }
            }
            return Ok(true);
        }
        if i.data.custom_id.starts_with("vent_panel:submit:")
            || i.data.custom_id.starts_with("vents:submit:")
        {
            let banned: i64 = sqlx::query(
                "SELECT EXISTS(SELECT 1 FROM vent_bans WHERE guild_id=? AND user_id=?)",
            )
            .bind(guild.get() as i64)
            .bind(i.user.id.get() as i64)
            .fetch_one(&data.vents_db)
            .await?
            .try_get(0)?;
            if banned != 0 {
                respond(ctx, i, "You are banned from using the vents system.").await?;
                return Ok(true);
            }
            if let Some(wait) =
                cooldown_remaining(&data.vents_db, guild.get(), i.user.id.get()).await?
            {
                respond(
                    ctx,
                    i,
                    &format!("Please wait **{wait}s** before submitting again."),
                )
                .await?;
                return Ok(true);
            }
            let modal =
                serenity::CreateModal::new("vents:submit_modal", "Submit an anonymous vent")
                    .components(vec![serenity::CreateActionRow::InputText(
                        CreateInputText::new(
                            InputTextStyle::Paragraph,
                            "Your vent",
                            "Write what is on your mind...",
                        )
                        .custom_id("body")
                        .required(true)
                        .max_length(2000),
                    )]);
            i.create_response(ctx, CreateInteractionResponse::Modal(modal))
                .await?;
            return Ok(true);
        }
        if let Some(id) = i
            .data
            .custom_id
            .strip_prefix("vent_reply:")
            .or_else(|| i.data.custom_id.strip_prefix("vents:reply:"))
            .and_then(|v| v.parse::<i64>().ok())
        {
            let banned: i64 = sqlx::query(
                "SELECT EXISTS(SELECT 1 FROM vent_bans WHERE guild_id=? AND user_id=?)",
            )
            .bind(guild.get() as i64)
            .bind(i.user.id.get() as i64)
            .fetch_one(&data.vents_db)
            .await?
            .try_get(0)?;
            if banned != 0 {
                respond(ctx, i, "You are banned from using the vents system.").await?;
                return Ok(true);
            }
            if let Some(wait) =
                cooldown_remaining(&data.vents_db, guild.get(), i.user.id.get()).await?
            {
                respond(
                    ctx,
                    i,
                    &format!("Please wait **{wait}s** before posting again."),
                )
                .await?;
                return Ok(true);
            }
            let modal =
                serenity::CreateModal::new(format!("vents:reply_modal:{id}"), "Reply anonymously")
                    .components(vec![serenity::CreateActionRow::InputText(
                        CreateInputText::new(
                            InputTextStyle::Paragraph,
                            "Your reply",
                            "Write a supportive reply...",
                        )
                        .custom_id("body")
                        .required(true)
                        .max_length(2000),
                    )]);
            i.create_response(ctx, CreateInteractionResponse::Modal(modal))
                .await?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn handle_modal(
    ctx: &serenity::Context,
    data: &Data,
    i: &serenity::all::ModalInteraction,
) -> Result<(), Error> {
    let cid = &i.data.custom_id;
    let Some(guild) = i.guild_id else {
        return Ok(());
    };
    let is_reply = cid.starts_with("vents:reply_modal:");
    if cid != "vents:submit_modal" && !is_reply {
        return Ok(());
    }
    let content = i
        .data
        .components
        .iter()
        .flat_map(|r| r.components.iter())
        .find_map(|c| match c {
            serenity::all::ActionRowComponent::InputText(input) => input.value.clone(),
            _ => None,
        })
        .unwrap_or_default();
    if content.trim().is_empty() {
        respond_modal(ctx, i, "Please enter some text.").await?;
        return Ok(());
    }
    let ban: i64 =
        sqlx::query("SELECT EXISTS(SELECT 1 FROM vent_bans WHERE guild_id=? AND user_id=?)")
            .bind(guild.get() as i64)
            .bind(i.user.id.get() as i64)
            .fetch_one(&data.vents_db)
            .await?
            .try_get(0)?;
    if ban != 0 {
        respond_modal(ctx, i, "You are banned from using the vents system.").await?;
        return Ok(());
    }
    let config = sqlx::query(
        "SELECT vent_channel_id,log_channel_id,cooldown_seconds FROM vent_config WHERE guild_id=?",
    )
    .bind(guild.get() as i64)
    .fetch_optional(&data.vents_db)
    .await?;
    let Some(config) = config else {
        respond_modal(ctx, i, "Vents are not configured.").await?;
        return Ok(());
    };
    if let Some(wait) = cooldown_remaining(&data.vents_db, guild.get(), i.user.id.get()).await? {
        respond_modal(
            ctx,
            i,
            &format!("Please wait {wait} seconds before posting again."),
        )
        .await?;
        return Ok(());
    }
    let entry_number: Option<i64> = if is_reply {
        None
    } else {
        Some(
            sqlx::query_scalar("SELECT COUNT(*)+1 FROM vents WHERE guild_id=?")
                .bind(guild.get() as i64)
                .fetch_one(&data.vents_db)
                .await?,
        )
    };
    let public = content
        .replace("@everyone", "@\u{200b}everyone")
        .replace("@here", "@\u{200b}here")
        .replace("<@", "<@\u{200b}");
    let (dest, vent_id) = if is_reply {
        let vent_message_id = cid
            .strip_prefix("vents:reply_modal:")
            .unwrap()
            .parse::<i64>();
        let Ok(vent_message_id) = vent_message_id else {
            respond_modal(ctx, i, "Invalid vent reference.").await?;
            return Ok(());
        };
        let row = sqlx::query(
            "SELECT id,thread_id,channel_id,message_id FROM vents WHERE message_id=? AND guild_id=? AND status='active'",
        )
        .bind(vent_message_id)
        .bind(guild.get() as i64)
        .fetch_optional(&data.vents_db)
        .await?;
        let Some(row) = row else {
            respond_modal(ctx, i, "This vent is no longer available.").await?;
            return Ok(());
        };
        let vent_id = row.try_get::<i64, _>("id")?;
        let mut thread = row.try_get::<Option<i64>, _>("thread_id")?;
        if thread.is_none() {
            let parent_channel = row.try_get::<i64, _>("channel_id")? as u64;
            let parent_message = row.try_get::<i64, _>("message_id")? as u64;
            thread = serenity::ChannelId::new(parent_channel)
                .create_thread_from_message(
                    ctx,
                    serenity::MessageId::new(parent_message),
                    serenity::CreateThread::new("Anonymous Replies")
                        .auto_archive_duration(serenity::AutoArchiveDuration::OneWeek),
                )
                .await
                .ok()
                .map(|t| t.id.get() as i64);
            if let Some(thread_id) = thread {
                sqlx::query("UPDATE vents SET thread_id=? WHERE id=?")
                    .bind(thread_id)
                    .bind(vent_id)
                    .execute(&data.vents_db)
                    .await?;
            }
        }
        let Some(thread_id) = thread else {
            respond_modal(ctx, i, "Could not locate or create the vent reply thread.").await?;
            return Ok(());
        };
        (thread_id as u64, Some(vent_id))
    } else {
        (
            config
                .try_get::<Option<i64>, _>("vent_channel_id")?
                .unwrap_or_default() as u64,
            None,
        )
    };
    let embed = CreateEmbed::new()
        .title(if is_reply {
            "Anonymous reply".to_owned()
        } else {
            format!("📔 Entry #{}", entry_number.unwrap_or_default())
        })
        .description(public)
        .color(0x800080)
        .footer(serenity::CreateEmbedFooter::new(if is_reply {
            "Anonymous reply"
        } else {
            "Reply anonymously using the button below"
        }))
        .timestamp(serenity::Timestamp::now());
    let mut message = serenity::ChannelId::new(dest)
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .embed(embed)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await?;
    let (log_type, record_id, summary) = if let Some(vent_id) = vent_id {
        let result = sqlx::query("INSERT INTO vent_replies(vent_id,message_id,author_id,content,created_at) VALUES(?,?,?,?,?)").bind(vent_id).bind(message.id.get() as i64).bind(i.user.id.get() as i64).bind(&content).bind(chrono::Utc::now().timestamp()).execute(&data.vents_db).await?;
        (
            "reply",
            result.last_insert_rowid(),
            format!("Anonymous reply to vent `{vent_id}`"),
        )
    } else {
        let thread = serenity::ChannelId::new(dest)
            .create_thread_from_message(
                ctx,
                message.id,
                serenity::CreateThread::new(format!("Replies · {}", message.id.get()))
                    .auto_archive_duration(serenity::AutoArchiveDuration::OneWeek),
            )
            .await
            .ok();
        let result = sqlx::query("INSERT INTO vents(guild_id,message_id,channel_id,thread_id,author_id,content,created_at) VALUES(?,?,?,?,?,?,?)").bind(guild.get() as i64).bind(message.id.get() as i64).bind(dest as i64).bind(thread.map(|t| t.id.get() as i64)).bind(i.user.id.get() as i64).bind(&content).bind(chrono::Utc::now().timestamp()).execute(&data.vents_db).await?;
        let entry: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM vents WHERE guild_id=? AND id<=?")
                .bind(guild.get() as i64)
                .bind(result.last_insert_rowid())
                .fetch_one(&data.vents_db)
                .await?;
        (
            "vent",
            result.last_insert_rowid(),
            format!(
                "New vent — **Entry #{entry}** (record `{}`)",
                result.last_insert_rowid()
            ),
        )
    };
    if !is_reply {
        let builder = vec![CreateActionRow::Buttons(vec![CreateButton::new(format!(
            "vent_reply:{}",
            message.id.get()
        ))
        .label("Reply anonymously")
        .style(ButtonStyle::Secondary)])];
        let _ = message
            .edit(ctx, serenity::EditMessage::new().components(builder))
            .await;
    }
    sqlx::query("INSERT INTO vent_cooldowns(guild_id,user_id,last_action_at) VALUES(?,?,?) ON CONFLICT(guild_id,user_id) DO UPDATE SET last_action_at=excluded.last_action_at").bind(guild.get() as i64).bind(i.user.id.get() as i64).bind(chrono::Utc::now().to_rfc3339()).execute(&data.vents_db).await?;
    if let Some(log) = config.try_get::<Option<i64>, _>("log_channel_id")? {
        let _ = send_log(
            ctx,
            guild.get(),
            dest,
            log as u64,
            log_type,
            record_id,
            &summary,
            message.id.get(),
        )
        .await;
    }
    if !is_reply {
        let panel_ctx = ctx.clone();
        let panel_data = data.clone();
        let panel_channel = serenity::ChannelId::new(dest);
        tokio::spawn(async move {
            if let Err(error) =
                stick_panel(&panel_ctx, &panel_data, guild, panel_channel, false).await
            {
                tracing::warn!("Could not refresh sticky vents panel: {error}");
            }
        });
    }
    respond_modal(
        ctx,
        i,
        if is_reply {
            "Your reply was posted anonymously."
        } else {
            "Your vent was posted anonymously."
        },
    )
    .await?;
    Ok(())
}

async fn send_log(
    ctx: &serenity::Context,
    guild: u64,
    public_channel: u64,
    channel: u64,
    kind: &str,
    record_id: i64,
    summary: &str,
    message_id: u64,
) -> Result<(), Error> {
    let title = "🔒 Vent Log Entry";
    let text=format!("{summary}\n\n[Jump to public post](https://discord.com/channels/{guild}/{public_channel}/{message_id})\n\nDetails are hidden for safety. Use the buttons to reveal or take action.");
    let buttons = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("vent_log:reveal:{kind}:{record_id}"))
            .label("Reveal Details")
            .emoji('👁')
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("vent_log:remove:{kind}:{record_id}"))
            .label("Remove Post")
            .emoji('🗑')
            .style(ButtonStyle::Primary),
        CreateButton::new(format!("vent_log:ban:{kind}:{record_id}"))
            .label("Ban from Vents")
            .emoji('🔨')
            .style(ButtonStyle::Danger),
    ])];
    let embed = CreateEmbed::new()
        .title(title)
        .description(text)
        .color(0x8B0000)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Type: {kind} | Record: {record_id}"
        )))
        .timestamp(serenity::Timestamp::now());
    serenity::ChannelId::new(channel)
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .embed(embed)
                .components(buttons)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await?;
    Ok(())
}

async fn respond(
    ctx: &serenity::Context,
    i: &serenity::all::ComponentInteraction,
    text: &str,
) -> Result<(), Error> {
    i.create_response(
        ctx,
        CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(text)
                .ephemeral(true),
        ),
    )
    .await?;
    Ok(())
}
async fn respond_modal(
    ctx: &serenity::Context,
    i: &serenity::all::ModalInteraction,
    text: &str,
) -> Result<(), Error> {
    i.create_response(
        ctx,
        CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new()
                .content(text)
                .ephemeral(true),
        ),
    )
    .await?;
    Ok(())
}

async fn cooldown_remaining(
    db: &sqlx::SqlitePool,
    guild: u64,
    user: u64,
) -> Result<Option<i64>, Error> {
    let row=sqlx::query("SELECT c.cooldown_seconds,d.last_action_at FROM vent_config c LEFT JOIN vent_cooldowns d ON d.guild_id=c.guild_id AND d.user_id=? WHERE c.guild_id=?").bind(user as i64).bind(guild as i64).fetch_optional(db).await?;
    let Some(row) = row else { return Ok(None) };
    let cooldown = row.try_get::<i64, _>("cooldown_seconds")?;
    let last = row.try_get::<Option<String>, _>("last_action_at")?;
    let Some(last) = last else { return Ok(None) };
    let timestamp = chrono::DateTime::parse_from_rfc3339(&last)
        .map(|date| date.timestamp())
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&last, "%Y-%m-%d %H:%M:%S")
                .map(|date| date.and_utc().timestamp())
        });
    let Ok(timestamp) = timestamp else {
        return Ok(None);
    };
    let elapsed = chrono::Utc::now().timestamp() - timestamp;
    Ok(Some((cooldown - elapsed).max(0)).filter(|n| *n > 0))
}
