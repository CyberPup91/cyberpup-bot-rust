pub mod database;
pub mod management;

use crate::{Data, Error};
use chrono::Utc;
use poise::serenity_prelude as serenity;
use serenity::Mentionable;

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    database::init_db(pool).await
}

pub async fn member_remove(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    user: &serenity::User,
) -> Result<(), Error> {
    if user.bot || !database::sticky_enabled(&data.moderation_db, guild_id.get()).await? {
        return Ok(());
    }
    let member = ctx
        .cache
        .guild(guild_id)
        .and_then(|guild| guild.members.get(&user.id).cloned());
    if let Some(member) = member {
        save_member_roles(ctx, data, guild_id, &member).await?;
    }
    Ok(())
}

pub async fn member_join(
    ctx: &serenity::Context,
    data: &Data,
    member: &serenity::Member,
) -> Result<(), Error> {
    if member.pending {
        return Ok(());
    }
    restore_sticky_roles(ctx, data, member.guild_id, member.user.id).await
}

pub async fn member_update(
    ctx: &serenity::Context,
    data: &Data,
    old: Option<&serenity::Member>,
    member: &serenity::Member,
) -> Result<(), Error> {
    if let Some(old) = old {
        if old.roles != member.roles
            && database::sticky_enabled(&data.moderation_db, member.guild_id.get()).await?
        {
            save_member_roles(ctx, data, member.guild_id, member).await?;
        }
        if old.pending && !member.pending {
            restore_sticky_roles(ctx, data, member.guild_id, member.user.id).await?;
        }
    } else if !member.pending {
        restore_sticky_roles(ctx, data, member.guild_id, member.user.id).await?;
    }
    Ok(())
}

async fn save_member_roles(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    member: &serenity::Member,
) -> Result<(), Error> {
    let blocked = database::blacklisted_roles(&data.moderation_db, guild_id.get())
        .await?
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let guild = ctx.cache.guild(guild_id).map(|guild| (*guild).clone());
    let role_ids = member
        .roles
        .iter()
        .map(|role| role.get())
        .filter(|id| !blocked.contains(id))
        .filter(|id| {
            guild
                .as_ref()
                .and_then(|guild| guild.roles.get(&serenity::RoleId::new(*id)))
                .is_some_and(|role| !role.managed)
        })
        .collect::<Vec<_>>();
    database::snapshot(
        &data.moderation_db,
        guild_id.get(),
        member.user.id.get(),
        &role_ids,
    )
    .await?;
    Ok(())
}

async fn restore_sticky_roles(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
) -> Result<(), Error> {
    if !database::sticky_enabled(&data.moderation_db, guild_id.get()).await? {
        return Ok(());
    }
    let Some(snapshot) =
        database::get_snapshot(&data.moderation_db, guild_id.get(), user_id.get()).await?
    else {
        return Ok(());
    };
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let guild = ctx.cache.guild(guild_id).map(|guild| (*guild).clone());
    let bot_id = ctx.cache.current_user().id;
    let bot_member = guild_id.member(ctx, bot_id).await?;
    let bot_top_position = bot_member
        .roles
        .iter()
        .filter_map(|id| guild.as_ref()?.roles.get(id).map(|role| role.position))
        .max()
        .unwrap_or(0);
    let member = guild_id.member(ctx, user_id).await?;
    let current = member
        .roles
        .iter()
        .map(|role| role.get())
        .collect::<std::collections::HashSet<_>>();
    let mut restored = Vec::new();
    let mut failed = Vec::new();
    for id in &snapshot.role_ids {
        let role_id = serenity::RoleId::new(*id);
        let Some(role) = guild.as_ref().and_then(|guild| guild.roles.get(&role_id)) else {
            continue;
        };
        if role.managed || role.position >= bot_top_position || current.contains(id) {
            failed.push(role.name.clone());
            continue;
        }
        match member.add_role(ctx, role_id).await {
            Ok(()) => restored.push(role.name.clone()),
            Err(_) => failed.push(role.name.clone()),
        }
    }
    if let Some(log_id) = database::log_channel(&data.moderation_db, guild_id.get()).await? {
        if !restored.is_empty() || !failed.is_empty() {
            let away = (Utc::now().timestamp() - snapshot.timestamp).max(0) as f64 / 3600.0;
            let embed = serenity::CreateEmbed::new()
                .title("🔒 Sticky Roles Reassigned")
                .color(0x5865F2)
                .field("Member", user_id.mention().to_string(), true)
                .field(
                    "Roles Restored",
                    if restored.is_empty() {
                        "None".into()
                    } else {
                        restored.join(", ")
                    },
                    false,
                )
                .field("Time Away", format!("{away:.1} hours"), true);
            let embed = if failed.is_empty() {
                embed
            } else {
                embed.field("Failed to Restore", failed.join("; "), false)
            };
            let _ = serenity::ChannelId::new(log_id)
                .send_message(ctx, serenity::CreateMessage::new().embed(embed))
                .await;
        }
    }
    database::remove_snapshot(&data.moderation_db, guild_id.get(), user_id.get()).await?;
    Ok(())
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<bool, Error> {
    if !interaction.data.custom_id.starts_with("moderation:") {
        return Ok(false);
    }
    management::handle_component(ctx, data, interaction).await?;
    Ok(true)
}
