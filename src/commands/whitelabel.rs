use crate::{Context, Error};
use base64::Engine;
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use std::path::Path;

const CONFIG_PATH: &str = "data/whitelabel_config.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum ActivityTypeChoice {
    #[name = "Playing"]
    Playing,
    #[name = "Streaming"]
    Streaming,
    #[name = "Listening"]
    Listening,
    #[name = "Watching"]
    Watching,
    #[name = "Competing"]
    Competing,
    #[name = "Custom Status"]
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum StatusChoice {
    #[name = "Online"]
    Online,
    #[name = "Idle"]
    Idle,
    #[name = "Do Not Disturb"]
    Dnd,
    #[name = "Invisible / Offline"]
    Invisible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelabelConfig {
    pub main_guild: u64,
    pub main_role: u64,
    pub disallowed_msg: String,
    pub status: String,
    pub activity_type: String,
    pub activity_text: String,
    pub activity_url: String,
}

impl Default for WhitelabelConfig {
    fn default() -> Self {
        Self {
            main_guild: 0,
            main_role: 0,
            disallowed_msg: String::new(),
            status: "online".to_string(),
            activity_type: "playing".to_string(),
            activity_text: String::new(),
            activity_url: String::new(),
        }
    }
}

pub async fn load_config() -> WhitelabelConfig {
    if !Path::new(CONFIG_PATH).exists() {
        return WhitelabelConfig::default();
    }
    match tokio::fs::read_to_string(CONFIG_PATH).await {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => WhitelabelConfig::default(),
    }
}

pub async fn save_config(cfg: &WhitelabelConfig) -> Result<(), Error> {
    if let Some(parent) = Path::new(CONFIG_PATH).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    tokio::fs::write(CONFIG_PATH, json).await?;
    Ok(())
}

pub async fn apply_saved_presence(
    ctx: &serenity::Context,
    cfg: &WhitelabelConfig,
) -> Result<(), Error> {
    let status = match cfg.status.to_lowercase().as_str() {
        "idle" => serenity::OnlineStatus::Idle,
        "dnd" => serenity::OnlineStatus::DoNotDisturb,
        "invisible" => serenity::OnlineStatus::Invisible,
        _ => serenity::OnlineStatus::Online,
    };

    let activity = if !cfg.activity_text.is_empty() {
        match cfg.activity_type.to_lowercase().as_str() {
            "listening" => Some(serenity::ActivityData::listening(&cfg.activity_text)),
            "watching" => Some(serenity::ActivityData::watching(&cfg.activity_text)),
            "competing" => Some(serenity::ActivityData::competing(&cfg.activity_text)),
            "streaming" => Some(serenity::ActivityData::streaming(
                &cfg.activity_text,
                if cfg.activity_url.is_empty() {
                    "https://www.twitch.tv/"
                } else {
                    &cfg.activity_url
                },
            )?),
            "custom" => Some(serenity::ActivityData::custom(&cfg.activity_text)),
            _ => Some(serenity::ActivityData::playing(&cfg.activity_text)),
        }
    } else {
        None
    };

    ctx.set_presence(activity, status);
    Ok(())
}

pub async fn is_allowed(
    ctx: Context<'_>,
    _guild_id: serenity::GuildId,
) -> Result<(bool, String), Error> {
    let cfg = load_config().await;
    if cfg.main_guild == 0 || cfg.main_role == 0 {
        return Ok((true, String::new()));
    }

    let main_guild_id = serenity::GuildId::new(cfg.main_guild);
    let main_role_id = serenity::RoleId::new(cfg.main_role);

    let guild = match ctx.partial_guild().await {
        Some(g) => g,
        None => return Ok((false, "Could not fetch server information.".into())),
    };

    let owner_id = guild.owner_id;

    let main_member = match ctx.http().get_member(main_guild_id, owner_id).await {
        Ok(m) => m,
        Err(_) => {
            return Ok((
                false,
                "The owner of this server is not in the main server!".into(),
            ));
        }
    };

    if !main_member.roles.contains(&main_role_id) {
        let msg = if cfg.disallowed_msg.is_empty() {
            "The owner of this server does not have the required role in the main server."
                .to_string()
        } else {
            cfg.disallowed_msg
                .replace("{owner_mention}", &format!("<@{owner_id}>"))
                .replace("{owner_name}", &main_member.user.name)
                .replace("{role_name}", &main_role_id.get().to_string())
                .replace("{main_guild}", &main_guild_id.get().to_string())
        };
        return Ok((false, msg));
    }

    Ok((true, String::new()))
}

async fn patch_guild_bot_profile(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    payload: &serde_json::Value,
) -> Result<(), Error> {
    let client = reqwest::Client::new();
    let url = format!("https://discord.com/api/v10/guilds/{guild_id}/members/@me");

    let response = client
        .patch(&url)
        .header("Authorization", ctx.http().token())
        .header("Content-Type", "application/json")
        .json(payload)
        .send()
        .await?;

    if !response.status().is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("Discord REST Error ({err_text})").into());
    }

    Ok(())
}

// ------------------------------------------------------------------
// Subcommands for /botprofile
// ------------------------------------------------------------------

#[poise::command(slash_command)]
pub async fn status(
    ctx: Context<'_>,
    #[description = "Activity text message"] text: Option<String>,
    #[description = "Activity Type"] activity_type: Option<ActivityTypeChoice>,
    #[description = "Online Status"] status: Option<StatusChoice>,
    #[description = "Stream URL for Streaming type"] url: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?;
    let (allowed, msg) = is_allowed(ctx, guild_id).await?;
    if !allowed {
        ctx.say(msg).await?;
        return Ok(());
    }

    let mut cfg = load_config().await;
    if let Some(s) = status {
        cfg.status = match s {
            StatusChoice::Online => "online".to_string(),
            StatusChoice::Idle => "idle".to_string(),
            StatusChoice::Dnd => "dnd".to_string(),
            StatusChoice::Invisible => "invisible".to_string(),
        };
    }
    if let Some(at) = activity_type {
        cfg.activity_type = match at {
            ActivityTypeChoice::Playing => "playing".to_string(),
            ActivityTypeChoice::Streaming => "streaming".to_string(),
            ActivityTypeChoice::Listening => "listening".to_string(),
            ActivityTypeChoice::Watching => "watching".to_string(),
            ActivityTypeChoice::Competing => "competing".to_string(),
            ActivityTypeChoice::Custom => "custom".to_string(),
        };
    }
    cfg.activity_text = text.unwrap_or_default();
    cfg.activity_url = url.unwrap_or_default();
    save_config(&cfg).await?;

    apply_saved_presence(ctx.serenity_context(), &cfg).await?;

    ctx.say(format!(
        "Updated global presence to status: **{}**!",
        cfg.status
    ))
    .await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn avatar(
    ctx: Context<'_>,
    #[description = "Avatar attachment (leave empty to reset)"] avatar: Option<
        serenity::Attachment,
    >,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?;
    let (allowed, msg) = is_allowed(ctx, guild_id).await?;
    if !allowed {
        ctx.say(msg).await?;
        return Ok(());
    }

    let avatar_data = if let Some(att) = avatar {
        if att.size > 8 * 1024 * 1024 {
            ctx.say("Attachment max size is 8MB.").await?;
            return Ok(());
        }
        let bytes = att.download().await?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mime = att.content_type.unwrap_or_else(|| "image/png".to_string());
        format!("data:{mime};base64,{b64}")
    } else {
        String::new()
    };

    let payload = serde_json::json!({ "avatar": avatar_data });
    patch_guild_bot_profile(ctx, guild_id, &payload).await?;

    ctx.say("Server avatar updated successfully!").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn banner(
    ctx: Context<'_>,
    #[description = "Banner attachment (leave empty to reset)"] banner: Option<
        serenity::Attachment,
    >,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?;
    let (allowed, msg) = is_allowed(ctx, guild_id).await?;
    if !allowed {
        ctx.say(msg).await?;
        return Ok(());
    }

    let banner_data = if let Some(att) = banner {
        if att.size > 10 * 1024 * 1024 {
            ctx.say("Attachment max size is 10MB.").await?;
            return Ok(());
        }
        let bytes = att.download().await?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let mime = att.content_type.unwrap_or_else(|| "image/png".to_string());
        format!("data:{mime};base64,{b64}")
    } else {
        String::new()
    };

    let payload = serde_json::json!({ "banner": banner_data });
    patch_guild_bot_profile(ctx, guild_id, &payload).await?;

    ctx.say("Server banner updated successfully!").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn bio(
    ctx: Context<'_>,
    #[description = "Server bio for the bot (leave empty to reset)"] bio: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?;
    let (allowed, msg) = is_allowed(ctx, guild_id).await?;
    if !allowed {
        ctx.say(msg).await?;
        return Ok(());
    }

    if let Some(ref b) = bio {
        if b.len() > 190 {
            ctx.say("Bio cannot be longer than 190 characters.").await?;
            return Ok(());
        }
    }

    let payload = serde_json::json!({ "bio": bio.unwrap_or_default() });
    patch_guild_bot_profile(ctx, guild_id, &payload).await?;

    ctx.say("Server bio updated successfully!").await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn nickname(
    ctx: Context<'_>,
    #[description = "Nickname (leave empty to reset)"] nickname: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?;
    let (allowed, msg) = is_allowed(ctx, guild_id).await?;
    if !allowed {
        ctx.say(msg).await?;
        return Ok(());
    }

    ctx.http()
        .edit_nickname(guild_id, nickname.as_deref(), None)
        .await?;

    ctx.say("Server nickname updated!").await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    subcommands("status", "avatar", "banner", "bio", "nickname")
)]
pub async fn botprofile(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

// ------------------------------------------------------------------
// Subcommands for /whitelabel
// ------------------------------------------------------------------

#[poise::command(slash_command)]
pub async fn view(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let cfg = load_config().await;

    let embed = serenity::CreateEmbed::default()
        .title("Whitelabel Configuration")
        .color(0x7289da)
        .field("Main Guild ID", cfg.main_guild.to_string(), true)
        .field("Main Role ID", cfg.main_role.to_string(), true)
        .field(
            "Status / Presence",
            format!(
                "Status: `{}` | Type: `{}`\nText: `{}`",
                cfg.status, cfg.activity_type, cfg.activity_text
            ),
            false,
        )
        .field(
            "Disallowed Msg",
            if cfg.disallowed_msg.is_empty() {
                "Default".to_string()
            } else {
                cfg.disallowed_msg
            },
            false,
        );

    ctx.send(poise::CreateReply::default().embed(embed)).await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn mainserver(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("Must be run in a server.")?;
    let mut cfg = load_config().await;
    cfg.main_guild = guild_id.get();
    cfg.main_role = 0;
    save_config(&cfg).await?;

    ctx.say(format!(
        "Main server set to: `{guild_id}`. Main role reset."
    ))
    .await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn mainrole(
    ctx: Context<'_>,
    #[description = "Required role in main server"] role: Option<serenity::Role>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let mut cfg = load_config().await;
    cfg.main_role = role.map(|r| r.id.get()).unwrap_or(0);
    save_config(&cfg).await?;

    ctx.say(format!("Main role set to ID: `{}`", cfg.main_role))
        .await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn disallowedmsg(
    ctx: Context<'_>,
    #[description = "Disallowed message template"] message: String,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let mut cfg = load_config().await;
    cfg.disallowed_msg = message;
    save_config(&cfg).await?;

    ctx.say("Disallowed message template updated!").await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "ADMINISTRATOR",
    subcommands("view", "mainserver", "mainrole", "disallowedmsg")
)]
pub async fn whitelabel(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}
