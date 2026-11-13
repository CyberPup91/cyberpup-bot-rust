use crate::{Context, Error};
use poise::serenity_prelude as serenity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum PurgeFilterChoice {
    #[name = "All messages"]
    All,
    #[name = "Embeds"]
    Embeds,
    #[name = "Files/Attachments"]
    Files,
    #[name = "Images"]
    Images,
    #[name = "Contains Text"]
    Contains,
    #[name = "Links"]
    Links,
    #[name = "Mentions"]
    Mentions,
    #[name = "Bot Users"]
    Bot,
    #[name = "Human Users"]
    Human,
    #[name = "Clear Reactions"]
    Reactions,
}

#[poise::command(slash_command, rename = "dump-ids")]
pub async fn dump_ids(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild = ctx.partial_guild().await.ok_or("Must be in a guild.")?;

    let mut output = String::from("=== SERVER ROLES ===\n");
    for role in guild.roles.values() {
        output.push_str(&format!("{}: {}\n", role.name, role.id));
    }

    output.push_str("\n=== SERVER CHANNELS ===\n");
    let channels = guild.channels(&ctx.http()).await?;
    for channel in channels.values() {
        output.push_str(&format!("{}: {}\n", channel.name, channel.id));
    }

    let attachment = serenity::CreateAttachment::bytes(output.into_bytes(), "server_ids.txt");
    ctx.send(
        poise::CreateReply::default()
            .content("Here is the complete list of all Role and Channel IDs:")
            .attachment(attachment)
            .ephemeral(true),
    )
    .await?;

    Ok(())
}

#[poise::command(slash_command, rename = "dump-emojis")]
pub async fn dump_emojis(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let bot_id = ctx.framework().bot_id();
    let url = format!("https://discord.com/api/v10/applications/{bot_id}/emojis");

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", ctx.http().token())
        .send()
        .await?;

    if !response.status().is_success() {
        ctx.say("🛑 Failed to fetch Application Emojis from Discord API.")
            .await?;
        return Ok(());
    }

    let json: serde_json::Value = response.json().await?;
    let items = json.get("items").and_then(|v| v.as_array());

    let mut lines = Vec::new();
    if let Some(emojis) = items {
        for e in emojis {
            let name = e.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let id = e.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let anim = if e.get("animated").and_then(|v| v.as_bool()).unwrap_or(false) {
                "a"
            } else {
                ""
            };
            lines.push(format!("<{anim}:{name}:{id}>"));
        }
    }

    if lines.is_empty() {
        ctx.say("No Application Emojis found on the Developer Portal.")
            .await?;
        return Ok(());
    }

    let content = lines.join("\n");
    let attachment = serenity::CreateAttachment::bytes(content.into_bytes(), "app_emojis.txt");

    ctx.send(
        poise::CreateReply::default()
            .content("Here are your formatted Application Emojis:")
            .attachment(attachment)
            .ephemeral(true),
    )
    .await?;

    Ok(())
}

#[poise::command(slash_command, rename = "toggle-status")]
pub async fn toggle_status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    ctx.serenity_context()
        .set_presence(None, serenity::OnlineStatus::Invisible);

    ctx.say("Bot status set to **Invisible** (appears offline).")
        .await?;
    Ok(())
}

#[poise::command(slash_command)]
pub async fn purge(
    ctx: Context<'_>,
    #[description = "Number of messages to search backwards"] count: u64,
    #[description = "Filter type"] filter_type: Option<PurgeFilterChoice>,
    #[description = "User target ID or mention"] member: Option<serenity::UserId>,
    #[description = "Text substring filter"] substring: Option<String>,
    #[description = "Simulate only (dry run)"] simulate: Option<bool>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let channel_id = ctx.channel_id();
    let is_simulation = simulate.unwrap_or(false);

    let messages = channel_id
        .messages(
            &ctx.http(),
            serenity::GetMessages::default().limit(count.min(100) as u8),
        )
        .await?;

    let filter = filter_type.unwrap_or(PurgeFilterChoice::All);
    let mut to_delete: Vec<serenity::Message> = Vec::new();

    for msg in messages {
        if msg.pinned {
            continue;
        }

        if let Some(target_user) = member {
            if msg.author.id != target_user {
                continue;
            }
        }

        if let Some(ref sub) = substring {
            if !msg.content.to_lowercase().contains(&sub.to_lowercase()) {
                continue;
            }
        }

        let matches = match filter {
            PurgeFilterChoice::All => true,
            PurgeFilterChoice::Embeds => !msg.embeds.is_empty(),
            PurgeFilterChoice::Files => !msg.attachments.is_empty(),
            PurgeFilterChoice::Images => msg.attachments.iter().any(|a| {
                a.content_type
                    .as_ref()
                    .is_some_and(|ct| ct.starts_with("image/"))
            }),
            PurgeFilterChoice::Contains => substring.is_some(),
            PurgeFilterChoice::Links => {
                msg.content.contains("http://") || msg.content.contains("https://")
            }
            PurgeFilterChoice::Mentions => !msg.mentions.is_empty(),
            PurgeFilterChoice::Bot => msg.author.bot,
            PurgeFilterChoice::Human => !msg.author.bot,
            PurgeFilterChoice::Reactions => !msg.reactions.is_empty(),
        };

        if matches {
            to_delete.push(msg);
        }
    }

    if filter == PurgeFilterChoice::Reactions {
        if !is_simulation {
            for msg in &to_delete {
                let _ = msg.delete_reactions(&ctx.http()).await;
            }
        }
        ctx.say(format!(
            "Cleared reactions from **{}** messages.",
            to_delete.len()
        ))
        .await?;
        return Ok(());
    }

    if !is_simulation && !to_delete.is_empty() {
        let msg_ids: Vec<serenity::MessageId> = to_delete.iter().map(|m| m.id).collect();
        channel_id.delete_messages(&ctx.http(), &msg_ids).await?;
    }

    let mode_str = if is_simulation {
        "[SIMULATION] Would purge"
    } else {
        "Successfully purged"
    };
    ctx.say(format!(
        "🧹 **{} {} messages** matching filter criteria.",
        mode_str,
        to_delete.len()
    ))
    .await?;

    Ok(())
}

#[poise::command(slash_command, rename = "sync")]
pub async fn sync_guild(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("Must be in a guild.")?;

    poise::builtins::register_in_guild(ctx, &ctx.framework().options().commands, guild_id).await?;
    ctx.say(format!("✅ Commands synced to server ID `{guild_id}`!"))
        .await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "ADMINISTRATOR",
    subcommands("dump_ids", "dump_emojis", "toggle_status", "purge", "sync_guild")
)]
pub async fn admin(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}
