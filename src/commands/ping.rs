use crate::{Context, Error};

/// Check whether the bot is responding.
#[poise::command(slash_command)]
pub async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("Pong! 🏓").await?;
    Ok(())
}
