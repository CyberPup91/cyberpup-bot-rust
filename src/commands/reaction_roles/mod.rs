pub mod database;
pub mod management;

use crate::{Data, Error};
use poise::serenity_prelude as serenity;
use std::collections::HashSet;

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    database::init_db(pool).await
}

pub async fn reaction_added(
    ctx: &serenity::Context,
    data: &Data,
    reaction: &serenity::all::Reaction,
) -> Result<(), Error> {
    let (Some(guild_id), Some(user_id)) = (reaction.guild_id, reaction.user_id) else {
        return Ok(());
    };
    let bot_id = ctx.cache.current_user().id;
    if user_id == bot_id {
        return Ok(());
    }
    let message_id = reaction.message_id.get();
    let Some(panel) = database::panel(&data.reaction_roles_db, message_id).await? else {
        return Ok(());
    };
    let emoji = reaction.emoji.to_string();
    let Some(role_id) =
        database::role_for_emoji(&data.reaction_roles_db, message_id, &emoji).await?
    else {
        return Ok(());
    };
    // Reaction-add events usually include the member; fetch when Discord omits it.
    let member = match reaction.member.clone() {
        Some(member) => member,
        None => match guild_id.member(ctx, user_id).await {
            Ok(member) => member,
            Err(error) => {
                tracing::warn!("Could not fetch reaction-role member: {error}");
                return Ok(());
            }
        },
    };
    assign_role(ctx, data, reaction, panel.max_selections, role_id, member).await
}

async fn assign_role(
    ctx: &serenity::Context,
    data: &Data,
    reaction: &serenity::all::Reaction,
    max_selections: u8,
    role_id: u64,
    member: serenity::Member,
) -> Result<(), Error> {
    let role_id = serenity::RoleId::new(role_id);
    let panel_roles =
        database::roles_for_message(&data.reaction_roles_db, reaction.message_id.get())
            .await?
            .into_iter()
            .map(serenity::RoleId::new)
            .collect::<HashSet<_>>();
    let selected = member
        .roles
        .iter()
        .copied()
        .filter(|role| panel_roles.contains(role))
        .collect::<Vec<_>>();
    if max_selections == 1 {
        let to_remove = selected
            .into_iter()
            .filter(|role| *role != role_id)
            .collect::<Vec<_>>();
        for old_role in to_remove {
            member.remove_role(ctx, old_role).await?;
        }
    } else if max_selections > 1
        && !selected.contains(&role_id)
        && selected.len() >= usize::from(max_selections)
    {
        let _ = reaction.delete(ctx).await;
        return Ok(());
    }
    if !member.roles.contains(&role_id) {
        member.add_role(ctx, role_id).await?;
    }
    Ok(())
}

pub async fn reaction_removed(
    ctx: &serenity::Context,
    data: &Data,
    reaction: &serenity::all::Reaction,
) -> Result<(), Error> {
    let (Some(guild_id), Some(user_id)) = (reaction.guild_id, reaction.user_id) else {
        return Ok(());
    };
    let bot_id = ctx.cache.current_user().id;
    if user_id == bot_id {
        return Ok(());
    }
    let Some(role_id) = database::role_for_emoji(
        &data.reaction_roles_db,
        reaction.message_id.get(),
        &reaction.emoji.to_string(),
    )
    .await?
    else {
        return Ok(());
    };
    let member = match guild_id.member(ctx, user_id).await {
        Ok(member) => member,
        Err(_) => return Ok(()),
    };
    member
        .remove_role(ctx, serenity::RoleId::new(role_id))
        .await?;
    Ok(())
}

pub async fn message_deleted(data: &Data, message_id: serenity::MessageId) -> Result<(), Error> {
    database::remove_panel(&data.reaction_roles_db, message_id.get()).await?;
    Ok(())
}

pub fn parse_bindings(body: &str) -> Vec<(String, u64)> {
    body.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let emoji = parts.next()?.trim().to_owned();
            if serenity::all::ReactionType::try_from(emoji.as_str()).is_err() {
                return None;
            }
            let mention_start = line.find("<@&")? + 3;
            let mention_end = line[mention_start..].find('>')? + mention_start;
            let role_id = line[mention_start..mention_end].parse().ok()?;
            Some((emoji, role_id))
        })
        .collect()
}

pub fn parse_color(color: Option<&str>) -> serenity::Colour {
    let Some(color) = color else {
        return serenity::Colour::BLURPLE;
    };
    let color = color.trim().trim_start_matches('#');
    u32::from_str_radix(color, 16)
        .map(serenity::Colour::new)
        .unwrap_or(serenity::Colour::BLURPLE)
}
