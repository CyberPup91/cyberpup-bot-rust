pub mod database;
pub mod management;
mod parser;

use crate::{Data, Error};
use poise::serenity_prelude as serenity;
use serenity::Mentionable;
use sqlx::Row;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

static CHANNEL_LOCKS: OnceLock<Mutex<HashMap<u64, Arc<Mutex<()>>>>> = OnceLock::new();

async fn channel_lock(channel_id: u64) -> Arc<Mutex<()>> {
    let locks = CHANNEL_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().await;
    locks
        .entry(channel_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    database::init_db(pool).await
}

pub async fn handle_message(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
) -> Result<(), Error> {
    let Some(guild_id) = message.guild_id else {
        return Ok(());
    };
    let lock = channel_lock(message.channel_id.get()).await;
    let _guard = lock.lock().await;
    let config = database::get_config(&data.counting_db, guild_id.get()).await?;
    if config.channel_id != Some(message.channel_id.get()) {
        return Ok(());
    }

    let content = message.content.trim();
    let Some(value) = parser::evaluate(content) else {
        delete_message(ctx, message).await;
        return Ok(());
    };
    let expected = config.current_count.saturating_add(1) as f64;
    let is_double_count = config.last_user_id == Some(message.author.id.get());
    let is_correct = (value - expected).abs() < f64::EPSILON && !is_double_count;
    let is_math = content.contains(['+', '-', '*', '/', '^', '%']) && is_correct;

    if is_correct {
        sqlx::query(
            "UPDATE guild_config SET current_count = ?, last_user_id = ? WHERE guild_id = ?",
        )
        .bind(expected as i64)
        .bind(message.author.id.get() as i64)
        .bind(guild_id.get() as i64)
        .execute(&data.counting_db)
        .await?;
        database::update_user_result(
            &data.counting_db,
            guild_id.get(),
            message.author.id.get(),
            config.base_reward + if is_math { config.math_bonus } else { 0 },
            true,
            is_math,
        )
        .await?;
        let _ = message
            .react(ctx, serenity::ReactionType::Unicode("✅".to_string()))
            .await;
        if let Some(role_id) = config.role_id {
            assign_counter_role(
                ctx,
                guild_id,
                role_id,
                message.author.id,
                config.last_user_id,
            )
            .await;
        }
    } else {
        delete_message(ctx, message).await;
        database::update_user_result(
            &data.counting_db,
            guild_id.get(),
            message.author.id.get(),
            -config.penalty_amount,
            false,
            false,
        )
        .await?;
        if config.reset_on_fail {
            sqlx::query(
                "UPDATE guild_config SET current_count = 0, last_user_id = NULL WHERE guild_id = ?",
            )
            .bind(guild_id.get() as i64)
            .execute(&data.counting_db)
            .await?;
        }
        let reason = if is_double_count {
            "You cannot count twice in a row!".to_string()
        } else {
            format!("Wrong number! Expected **{expected:.0}**.")
        };
        let text = if config.reset_on_fail {
            format!(
                "💥 {} ruined the count at **{}**! {} The count has been reset to **0**. Lost **{}** {} {} penalty.",
                message.author.mention(), config.current_count, reason, config.penalty_amount,
                config.currency_emoji, config.currency_name
            )
        } else {
            format!(
                "⚠️ {} made a mistake! {} Lost **{}** {} {} penalty.",
                message.author.mention(),
                reason,
                config.penalty_amount,
                config.currency_emoji,
                config.currency_name
            )
        };
        let warning = message
            .channel_id
            .send_message(ctx, serenity::CreateMessage::new().content(text))
            .await?;
        let http = ctx.http.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let _ = warning.delete(&http).await;
        });
    }
    Ok(())
}

async fn delete_message(ctx: &serenity::Context, message: &serenity::Message) {
    let _ = message.delete(ctx).await;
}

async fn assign_counter_role(
    ctx: &serenity::Context,
    guild_id: serenity::GuildId,
    role_id: u64,
    new_holder: serenity::UserId,
    previous_user: Option<u64>,
) {
    let role = serenity::RoleId::new(role_id);
    if let Some(previous_user) = previous_user.filter(|id| *id != new_holder.get()) {
        if let Ok(member) = guild_id
            .member(ctx, serenity::UserId::new(previous_user))
            .await
        {
            let _ = member.remove_role(ctx, role).await;
        }
    }
    if let Ok(member) = guild_id.member(ctx, new_holder).await {
        let _ = member.add_role(ctx, role).await;
    }
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<bool, Error> {
    if !interaction.data.custom_id.starts_with("counting:") {
        return Ok(false);
    }
    management::handle_component(ctx, data, interaction).await?;
    Ok(true)
}

pub async fn handle_modal(
    _ctx: &serenity::Context,
    _data: &Data,
    _interaction: &serenity::all::ModalInteraction,
) -> Result<(), Error> {
    Ok(())
}

pub fn start_background(ctx: serenity::Context, data: Data) {
    tokio::spawn(async move {
        loop {
            let rows = sqlx::query(
                "SELECT guild_id FROM guild_config WHERE leaderboard_channel_id IS NOT NULL",
            )
            .fetch_all(&data.counting_db)
            .await;
            if let Ok(rows) = rows {
                for row in rows {
                    let Ok(guild_id) = row.try_get::<i64, _>("guild_id") else {
                        continue;
                    };
                    if let Err(error) = update_leaderboard(&ctx, &data, guild_id as u64).await {
                        tracing::warn!(
                            "Could not refresh counting leaderboard for guild {guild_id}: {error}"
                        );
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        }
    });
}

pub async fn update_leaderboard(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: u64,
) -> Result<(), Error> {
    let config = database::get_config(&data.counting_db, guild_id).await?;
    let Some(channel_id) = config.leaderboard_channel_id else {
        return Ok(());
    };
    let rows = database::top_users(&data.counting_db, guild_id, 10).await?;
    let description = leaderboard_description(&rows, &config.currency_emoji, &config.currency_name);
    let embed = serenity::CreateEmbed::new()
        .title(format!(
            "🏆 Persistent Counting Leaderboard ({})",
            config.currency_name
        ))
        .description(description)
        .color(0xFFD700)
        .footer(serenity::CreateEmbedFooter::new(
            "Updates automatically every 5 minutes",
        ));
    let channel = serenity::ChannelId::new(channel_id);
    if let Some(message_id) = config.leaderboard_message_id {
        match channel
            .edit_message(
                ctx,
                serenity::MessageId::new(message_id),
                serenity::EditMessage::new()
                    .embed(embed.clone())
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
        {
            Ok(_) => return Ok(()),
            Err(serenity::Error::Http(error))
                if error.status_code() == Some(serenity::http::StatusCode::NOT_FOUND) => {}
            Err(error) => return Err(Box::new(error)),
        }
    }
    let sent = channel
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .embed(embed)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await?;
    sqlx::query("UPDATE guild_config SET leaderboard_message_id = ? WHERE guild_id = ?")
        .bind(sent.id.get() as i64)
        .bind(guild_id as i64)
        .execute(&data.counting_db)
        .await?;
    Ok(())
}

fn leaderboard_description(rows: &[database::UserStats], emoji: &str, name: &str) -> String {
    if rows.is_empty() {
        return "*No counting stats recorded yet!*".to_string();
    }
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            format!(
                "**{}. <@{}>** — {} {emoji} {name}\n└ *Counts:* {} | *Math Solved:* {}",
                index + 1,
                row.user_id,
                row.balance,
                row.counts_made,
                row.math_solved
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
