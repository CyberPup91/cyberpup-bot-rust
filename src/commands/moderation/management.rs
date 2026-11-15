use crate::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, RoleId, UserId,
};
use serenity::Mentionable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum DeleteMessages {
    #[name = "Don't delete any"]
    None,
    #[name = "Previous Hour"]
    Hour,
    #[name = "Previous 24 Hours"]
    Day,
    #[name = "Previous 7 Days"]
    Week,
}

#[poise::command(
    slash_command,
    guild_only,
    subcommands(
        "sticky_roles",
        "set_log_channel",
        "exclude_role",
        "include_role",
        "ban",
        "timeout"
    )
)]
pub async fn moderation(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Open the sticky-role settings panel.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "ADMINISTRATOR",
    rename = "sticky-roles"
)]
pub async fn sticky_roles(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?.get();
    let guild_name = ctx
        .guild()
        .map(|guild| guild.name.clone())
        .unwrap_or_else(|| "Server".into());
    let embed = sticky_embed(ctx.data(), guild_id, &guild_name).await?;
    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .components(sticky_components())
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Set the channel where moderation actions and sticky-role restores are logged.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    rename = "set-log-channel"
)]
pub async fn set_log_channel(
    ctx: Context<'_>,
    #[description = "Channel used for moderation logs"] channel: serenity::ChannelId,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?.get();
    super::database::set_log_channel(&ctx.data().moderation_db, guild_id, channel.get()).await?;
    ctx.say(format!(
        "✅ Moderation logs will be sent to {}.",
        channel.mention()
    ))
    .await?;
    Ok(())
}

/// Exclude a role from sticky-role snapshots.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "ADMINISTRATOR",
    rename = "exclude-role"
)]
pub async fn exclude_role(
    ctx: Context<'_>,
    #[description = "Role to exclude from snapshots"] role: RoleId,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?.get();
    super::database::blacklist_role(&ctx.data().moderation_db, guild_id, role.get(), true).await?;
    ctx.say(format!(
        "{} will be excluded from sticky-role restoration.",
        role.mention()
    ))
    .await?;
    Ok(())
}

/// Remove a role from the sticky-role exclusion list.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "ADMINISTRATOR",
    rename = "include-role"
)]
pub async fn include_role(
    ctx: Context<'_>,
    #[description = "Role to include in snapshots"] role: RoleId,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?.get();
    super::database::blacklist_role(&ctx.data().moderation_db, guild_id, role.get(), false).await?;
    ctx.say(format!(
        "{} will be included in sticky-role snapshots.",
        role.mention()
    ))
    .await?;
    Ok(())
}

/// Ban a user by mention or ID, including users outside the server.
#[poise::command(slash_command, guild_only, required_permissions = "BAN_MEMBERS")]
pub async fn ban(
    ctx: Context<'_>,
    #[description = "User to ban"] target: UserId,
    #[description = "Recent messages to delete"] delete_messages: DeleteMessages,
    #[description = "Reason for the ban"] reason: Option<String>,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?;
    let reason = reason.unwrap_or_else(|| "No reason provided".into());
    if let Ok(member) = guild_id.member(ctx, target).await {
        if !can_moderate(ctx.serenity_context(), guild_id, ctx.author().id, &member) {
            ctx.say("❌ You cannot ban someone with a role equal to or higher than yours.")
                .await?;
            return Ok(());
        }
    }
    let seconds = match delete_messages {
        DeleteMessages::None => 0,
        DeleteMessages::Hour => 3600,
        DeleteMessages::Day => 86400,
        DeleteMessages::Week => 604800,
    };
    ban_with_seconds(ctx, ctx.data(), guild_id, target, seconds, &reason).await?;
    let user = target.to_user(ctx).await?;
    send_mod_log(
        ctx.serenity_context(),
        ctx.data(),
        guild_id.get(),
        "Ban",
        &user,
        &reason,
        Some(format!("Deleted recent messages: {seconds} seconds")),
    )
    .await?;
    ctx.say(format!("✅ Successfully banned {}.", target.mention()))
        .await?;
    Ok(())
}

/// Timeout a member for a number of minutes.
#[poise::command(slash_command, guild_only, required_permissions = "MODERATE_MEMBERS")]
pub async fn timeout(
    ctx: Context<'_>,
    #[description = "Member to timeout"] target: UserId,
    #[description = "Duration in minutes (1–40320)"] minutes: u64,
    #[description = "Reason for the timeout"] reason: Option<String>,
) -> Result<(), Error> {
    if minutes == 0 || minutes > 40_320 {
        ctx.say("❌ Timeout duration must be between 1 minute and 28 days.")
            .await?;
        return Ok(());
    }
    let guild_id = ctx.guild_id().ok_or("Must be in a server.")?;
    let member = match guild_id.member(ctx, target).await {
        Ok(member) => member,
        Err(_) => {
            ctx.say("❌ Could not find that member in this server.")
                .await?;
            return Ok(());
        }
    };
    if !can_moderate(ctx.serenity_context(), guild_id, ctx.author().id, &member) {
        ctx.say("❌ You cannot timeout someone with a role equal to or higher than yours.")
            .await?;
        return Ok(());
    }
    let reason = reason.unwrap_or_else(|| "No reason provided".into());
    let until = serenity::Timestamp::from_unix_timestamp(
        chrono::Utc::now().timestamp() + (minutes * 60) as i64,
    )?;
    guild_id
        .edit_member(
            ctx,
            target,
            serenity::EditMember::new()
                .disable_communication_until_datetime(until)
                .audit_log_reason(&format!("Timed out by {}: {reason}", ctx.author().name)),
        )
        .await?;
    let user = target.to_user(ctx).await?;
    send_mod_log(
        ctx.serenity_context(),
        ctx.data(),
        guild_id.get(),
        "Timeout",
        &user,
        &reason,
        Some(format!("Duration: {minutes} minutes")),
    )
    .await?;
    ctx.say(format!(
        "✅ Successfully timed out {} for {minutes} minutes.",
        target.mention()
    ))
    .await?;
    Ok(())
}

async fn ban_with_seconds(
    ctx: Context<'_>,
    data: &Data,
    guild_id: serenity::GuildId,
    user_id: UserId,
    seconds: u32,
    reason: &str,
) -> Result<(), Error> {
    let encoded_reason = percent_encode(reason);
    let endpoint = format!("https://discord.com/api/v10/guilds/{guild_id}/bans/{user_id}?delete_message_seconds={seconds}");
    let response = data
        .http
        .put(endpoint)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bot {}", ctx.http().token()),
        )
        .header("X-Audit-Log-Reason", encoded_reason)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(format!(
            "Discord ban request failed with status {}",
            response.status()
        )
        .into());
    }
    Ok(())
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn can_moderate(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    actor_id: UserId,
    target: &serenity::Member,
) -> bool {
    let Some(guild) = ctx.cache.guild(guild_id) else {
        return false;
    };
    if actor_id == guild.owner_id {
        return true;
    }
    let actor = guild.members.get(&actor_id);
    let top_position = |member: &serenity::Member| {
        member
            .roles
            .iter()
            .filter_map(|id| guild.roles.get(id).map(|role| role.position))
            .max()
            .unwrap_or(0)
    };
    actor.is_some_and(|actor| top_position(actor) > top_position(target))
}

async fn send_mod_log(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: u64,
    action: &str,
    target: &serenity::User,
    reason: &str,
    extra: Option<String>,
) -> Result<(), Error> {
    let Some(channel) = super::database::log_channel(&data.moderation_db, guild_id).await? else {
        return Ok(());
    };
    let color = if action == "Ban" { 0xE74C3C } else { 0xE67E22 };
    let mut embed = CreateEmbed::new()
        .title(format!("🚨 Moderation Action: {action}"))
        .color(color)
        .field(
            "Target",
            format!("{}\n`{}`", target.id.mention(), target.id),
            true,
        )
        .field("Moderator", "Moderation command", true)
        .field("Reason", reason, false);
    if let Some(extra) = extra {
        embed = embed.field("Details", extra, false);
    }
    let embed = embed.thumbnail(target.avatar_url().unwrap_or_default());
    serenity::ChannelId::new(channel)
        .send_message(ctx, serenity::CreateMessage::new().embed(embed))
        .await?;
    Ok(())
}

fn sticky_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("moderation:sticky_enable")
            .label("Enable")
            .style(ButtonStyle::Success),
        CreateButton::new("moderation:sticky_disable")
            .label("Disable")
            .style(ButtonStyle::Danger),
        CreateButton::new("moderation:sticky_clear")
            .label("Clear Snapshots")
            .style(ButtonStyle::Secondary),
    ])]
}

async fn sticky_embed(data: &Data, guild_id: u64, guild_name: &str) -> Result<CreateEmbed, Error> {
    let enabled = super::database::sticky_enabled(&data.moderation_db, guild_id).await?;
    let roles = super::database::blacklisted_roles(&data.moderation_db, guild_id).await?;
    let count = super::database::snapshot_count(&data.moderation_db, guild_id).await?;
    let exclusions = if roles.is_empty() {
        "None excluded (all roles tracked).".into()
    } else {
        roles
            .iter()
            .map(|id| format!("<@&{id}> (`{id}`)"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(CreateEmbed::new().title("🔒 Sticky Roles Manager").color(0x5865F2)
        .description(format!("Status: {}\n\nWhen a member with roles leaves, their non-excluded roles are saved and restored when they rejoin.\n\nUse `/moderation exclude-role` and `/moderation include-role` to manage exclusions.", if enabled { "✅ Enabled" } else { "❌ Disabled" }))
        .field(format!("Excluded Roles ({})", roles.len()), exclusions, false)
        .field("Saved Snapshots", count.to_string(), true)
        .footer(serenity::CreateEmbedFooter::new(guild_name)))
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return Ok(());
    };
    let admin = interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(|permissions| permissions.administrator());
    if !admin {
        interaction
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Only administrators can change sticky-role settings.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }
    match interaction.data.custom_id.as_str() {
        "moderation:sticky_enable" | "moderation:sticky_disable" => {
            let enabled = interaction.data.custom_id.ends_with("enable");
            super::database::set_sticky_enabled(&data.moderation_db, guild_id, enabled).await?;
            let guild_name = ctx
                .cache
                .guild(serenity::GuildId::new(guild_id))
                .map(|guild| guild.name.clone())
                .unwrap_or_else(|| "Server".into());
            let embed = sticky_embed(data, guild_id, &guild_name).await?;
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(embed)
                            .components(sticky_components()),
                    ),
                )
                .await?;
        }
        "moderation:sticky_clear" => {
            sqlx::query("DELETE FROM sticky_roles_snapshots WHERE guild_id = ?")
                .bind(guild_id as i64)
                .execute(&data.moderation_db)
                .await?;
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("Sticky-role snapshots cleared.")
                            .ephemeral(true),
                    ),
                )
                .await?;
        }
        _ => {}
    }
    Ok(())
}
