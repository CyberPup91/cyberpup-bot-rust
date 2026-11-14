use crate::{Context, Error};
use poise::serenity_prelude as serenity;

/// Manage the bot's voice connection and voice channel status.
#[poise::command(slash_command, subcommands("join", "leave", "setstatus"))]
pub async fn vc(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Connect the bot to a voice channel.
#[poise::command(slash_command)]
pub async fn join(
    ctx: Context<'_>,
    #[description = "Voice channel to join (defaults to your current channel)"]
    #[channel_types("Voice")]
    channel: Option<serenity::ChannelId>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or("This command only works in a server.")?;

    let target_channel = match channel {
        Some(id) => id,
        None => ctx
            .guild()
            .and_then(|g| g.voice_states.get(&ctx.author().id)?.channel_id)
            .ok_or("You are not in a voice channel! Specify a channel or join one first.")?,
    };

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird Voice client not initialized.")?;

    if manager.get(guild_id).is_some() {
        let _ = manager.remove(guild_id).await;
    }

    let _handler = manager.join(guild_id, target_channel).await;
    ctx.say(format!("Joined <#{target_channel}>!")).await?;
    Ok(())
}

/// Disconnect the bot from its current voice channel.
#[poise::command(slash_command)]
pub async fn leave(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let guild_id = ctx
        .guild_id()
        .ok_or("This command only works in a server.")?;

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird Voice client not initialized.")?;

    if manager.get(guild_id).is_none() {
        ctx.say("I'm not in a voice channel in this server.")
            .await?;
        return Ok(());
    }

    manager.remove(guild_id).await?;
    ctx.say("Disconnected from voice channel.").await?;
    Ok(())
}

/// Set the status text shown on a voice channel.
#[poise::command(slash_command)]
pub async fn setstatus(
    ctx: Context<'_>,
    #[description = "New voice channel status text"] status: String,
    #[description = "Voice channel to update (defaults to bot's or your current VC)"]
    #[channel_types("Voice")]
    channel: Option<serenity::ChannelId>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let _guild_id = ctx
        .guild_id()
        .ok_or("This command only works in a server.")?;

    let bot_id = ctx.framework().bot_id();
    let bot_vc = ctx
        .guild()
        .and_then(|g| g.voice_states.get(&bot_id)?.channel_id);
    let user_vc = ctx
        .guild()
        .and_then(|g| g.voice_states.get(&ctx.author().id)?.channel_id);

    let target_channel = channel
        .or(bot_vc)
        .or(user_vc)
        .ok_or("Must specify a voice channel or be connected to one!")?;

    let client = reqwest::Client::new();
    let url = format!("https://discord.com/api/v10/channels/{target_channel}/voice-status");
    let body = serde_json::json!({ "status": status });

    let response = client
        .put(&url)
        .header("Authorization", ctx.http().token())
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("Discord API error: {err_text}").into());
    }

    ctx.say(format!(
        "Updated voice status for <#{target_channel}> to: **{status}**"
    ))
    .await?;
    Ok(())
}
