use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use serenity::Mentionable;

/// Configure message logging for selected users.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    subcommands("manage", "force_remove")
)]
pub async fn message_logger(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Set the log destination and choose which message events to record.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
pub async fn manage(
    ctx: Context<'_>,
    #[description = "Member to monitor"] user: serenity::User,
    #[description = "Channel or thread where logs should be sent"] destination: serenity::ChannelId,
    #[description = "Log sent messages"] sent: bool,
    #[description = "Log edited messages"] edited: bool,
    #[description = "Log deleted messages"] deleted: bool,
) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("This command requires a server.")?;
    super::database::save(
        &ctx.data().message_logger_db,
        guild_id.get(),
        user.id.get(),
        destination.get(),
        sent,
        edited,
        deleted,
    )
    .await?;
    ctx.send(
        poise::CreateReply::default()
            .content(format!(
        "Message logging updated for {}. Destination: {}. Sent: {}; edited: {}; deleted: {}.",
        user.mention(), destination.mention(), on_off(sent), on_off(edited), on_off(deleted)
    ))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Remove message logging configuration for a user ID.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    rename = "force-remove"
)]
pub async fn force_remove(
    ctx: Context<'_>,
    #[description = "Numeric user ID"] user_id: String,
) -> Result<(), Error> {
    let id = user_id
        .parse::<u64>()
        .map_err(|_| "User ID must be numeric.")?;
    let guild_id = ctx.guild_id().ok_or("This command requires a server.")?;
    let removed =
        super::database::remove(&ctx.data().message_logger_db, guild_id.get(), id).await?;
    ctx.send(
        poise::CreateReply::default()
            .content(if removed {
                format!("Removed logging settings for `{id}`.")
            } else {
                format!("No logging settings found for `{id}`.")
            })
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}
