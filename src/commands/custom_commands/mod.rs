pub mod database;
pub mod management;
mod parser;

use crate::{Data, Error};
use poise::serenity_prelude as serenity;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

type CooldownMap = HashMap<(u64, i64, Option<u64>), Instant>;

static COOLDOWNS: OnceLock<Mutex<CooldownMap>> = OnceLock::new();

fn cooldowns() -> &'static Mutex<CooldownMap> {
    COOLDOWNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn parse_ids(value: &str) -> Vec<u64> {
    value
        .split(',')
        .filter_map(|part| part.trim().parse::<u64>().ok())
        .collect()
}

fn has_access(message: &serenity::Message, allowed_users: &[u64], allowed_roles: &[u64]) -> bool {
    if allowed_users.is_empty() && allowed_roles.is_empty() {
        return true;
    }

    if allowed_users.contains(&message.author.id.get()) {
        return true;
    }

    message.member.as_ref().is_some_and(|member| {
        member
            .roles
            .iter()
            .any(|role| allowed_roles.contains(&role.get()))
    })
}

async fn check_cooldowns(
    guild_id: u64,
    command_id: i64,
    user_id: u64,
    global_seconds: i64,
    user_seconds: i64,
) -> bool {
    let mut map = cooldowns().lock().await;
    let now = Instant::now();

    let global_key = (guild_id, command_id, None);
    let user_key = (guild_id, command_id, Some(user_id));

    if global_seconds > 0
        && map
            .get(&global_key)
            .is_some_and(|last| last.elapsed() < Duration::from_secs(global_seconds as u64))
    {
        return false;
    }

    if user_seconds > 0
        && map
            .get(&user_key)
            .is_some_and(|last| last.elapsed() < Duration::from_secs(user_seconds as u64))
    {
        return false;
    }

    map.insert(global_key, now);
    map.insert(user_key, now);

    // Opportunistically remove old entries so this cache doesn't grow forever.
    map.retain(|_, last| last.elapsed() < Duration::from_secs(86_400));

    true
}

pub async fn handle_message(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
) -> Result<(), Error> {
    if message.author.bot {
        return Ok(());
    }

    let Some(guild_id) = message.guild_id else {
        return Ok(());
    };

    let Some(trigger) = message.content.split_whitespace().next() else {
        return Ok(());
    };

    let Some(command_name) = trigger.strip_prefix('!') else {
        return Ok(());
    };

    if command_name.is_empty() {
        return Ok(());
    }

    let Some(command) =
        database::get_command(&data.custom_commands_db, guild_id.get(), command_name).await?
    else {
        return Ok(());
    };

    let allowed_users = parse_ids(&command.allowed_users);
    let allowed_roles = parse_ids(&command.allowed_roles);

    if !has_access(message, &allowed_users, &allowed_roles) {
        return Ok(());
    }

    if !check_cooldowns(
        guild_id.get(),
        command.id,
        message.author.id.get(),
        command.cooldown,
        command.user_cooldown,
    )
    .await
    {
        return Ok(());
    }

    let count = if command.response.contains("${count}") {
        Some(
            database::increment_counter(&data.custom_commands_db, guild_id.get(), command.id)
                .await?,
        )
    } else {
        None
    };

    let response = parser::expand(&command.response, message, count);

    if command.should_reply {
        message.reply(ctx, response).await?;
    } else {
        message
            .channel_id
            .send_message(ctx, serenity::CreateMessage::new().content(response))
            .await?;
    }

    Ok(())
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<bool, Error> {
    if !interaction.data.custom_id.starts_with("cc:") {
        return Ok(false);
    }

    management::handle_component(ctx, data, interaction).await
}
