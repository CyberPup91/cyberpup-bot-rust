pub mod database;
pub mod management;

use crate::{Data, Error};
use poise::serenity_prelude as serenity;
use serenity::Mentionable;

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    database::init_db(pool).await
}

pub async fn log_sent(
    ctx: &serenity::Context,
    data: &Data,
    message: &serenity::Message,
) -> Result<(), Error> {
    let Some(guild_id) = message.guild_id else {
        return Ok(());
    };
    if message.author.bot {
        return Ok(());
    }
    let Some(settings) = database::get(
        &data.message_logger_db,
        guild_id.get(),
        message.author.id.get(),
    )
    .await?
    else {
        return Ok(());
    };
    if settings.log_deleted || settings.log_edited {
        database::save_snapshot(
            &data.message_logger_db,
            message.channel_id.get(),
            message.id.get(),
            &database::MessageSnapshot {
                guild_id: guild_id.get(),
                user_id: message.author.id.get(),
                author_name: message.author.name.clone(),
                avatar_url: message.author.face(),
                content: message.content.clone(),
                attachment_names: message
                    .attachments
                    .iter()
                    .map(|a| a.filename.clone())
                    .collect(),
                created_at: message.timestamp.to_string(),
            },
        )
        .await?;
    }
    if !settings.log_all {
        return Ok(());
    }
    let Some(channel_id) = settings.log_channel_id else {
        return Ok(());
    };
    let content = if message.content.is_empty() {
        "*No text content*"
    } else {
        &message.content
    };
    let mut embed = serenity::CreateEmbed::new()
        .description(format!(
            "📝 **Message sent in** {} • [Jump to Message]({})\n\n{}",
            message.channel_id.mention(),
            message.link(),
            content
        ))
        .color(serenity::Colour::DARK_GREEN)
        .timestamp(message.timestamp)
        .author(
            serenity::CreateEmbedAuthor::new(format!(
                "{} ({})",
                message.author.name, message.author.id
            ))
            .icon_url(message.author.face()),
        );
    embed = add_attachments(embed, &message.attachments);
    send_log(ctx, serenity::ChannelId::new(channel_id), embed).await
}

pub async fn log_edit(
    ctx: &serenity::Context,
    data: &Data,
    old: Option<&serenity::Message>,
    new: Option<&serenity::Message>,
    event: &serenity::all::MessageUpdateEvent,
) -> Result<(), Error> {
    let Some(guild_id) = event
        .guild_id
        .or_else(|| new.and_then(|message| message.guild_id))
    else {
        return Ok(());
    };
    let author = event
        .author
        .as_ref()
        .or_else(|| new.map(|message| &message.author));
    let Some(author) = author else { return Ok(()) };
    if author.bot {
        return Ok(());
    }
    let Some(settings) =
        database::get(&data.message_logger_db, guild_id.get(), author.id.get()).await?
    else {
        return Ok(());
    };
    if !settings.log_edited {
        return Ok(());
    }
    let Some(channel_id) = settings.log_channel_id else {
        return Ok(());
    };
    let snapshot = database::get_snapshot(
        &data.message_logger_db,
        event.channel_id.get(),
        event.id.get(),
    )
    .await?;
    let old_content = old
        .map(|message| message.content.as_str())
        .or_else(|| snapshot.as_ref().map(|message| message.content.as_str()));
    let after_content = if let Some(content) = event.content.as_deref() {
        content.to_owned()
    } else if let Some(new) = new {
        new.content.clone()
    } else {
        match ctx.http.get_message(event.channel_id, event.id).await {
            Ok(message) => message.content,
            Err(_) => return Ok(()),
        }
    };
    if old_content.is_some_and(|before| before == after_content) {
        return Ok(());
    }
    let before = old_content
        .filter(|s| !s.is_empty())
        .unwrap_or("*No text content*");
    let after = if after_content.is_empty() {
        "*No text content*"
    } else {
        &after_content
    };
    database::update_snapshot_content(
        &data.message_logger_db,
        event.channel_id.get(),
        event.id.get(),
        &after_content,
    )
    .await?;
    let jump_url = format!(
        "https://discord.com/channels/{}/{}/{}",
        guild_id, event.channel_id, event.id
    );
    let mut embed = serenity::CreateEmbed::new()
        .description(format!(
            "✏️ **Message edited in** {} • [Jump to Message]({})",
            event.channel_id.mention(),
            jump_url
        ))
        .color(serenity::Colour::ORANGE)
        .timestamp(serenity::Timestamp::now())
        .author(
            serenity::CreateEmbedAuthor::new(format!("{} ({})", author.name, author.id))
                .icon_url(author.face()),
        )
        .field("Before", truncate(before, 1024), false)
        .field("After", truncate(after, 1024), false);
    if let Some(attachments) = event
        .attachments
        .as_deref()
        .or_else(|| new.map(|message| message.attachments.as_slice()))
    {
        embed = add_attachments(embed, attachments);
    }
    send_log(ctx, serenity::ChannelId::new(channel_id), embed).await
}

pub async fn log_deleted_ids(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
) -> Result<(), Error> {
    let Some(snapshot) =
        database::get_snapshot(&data.message_logger_db, channel_id.get(), message_id.get()).await?
    else {
        return Ok(());
    };
    let Some(settings) =
        database::get(&data.message_logger_db, guild_id.get(), snapshot.user_id).await?
    else {
        return Ok(());
    };
    if !settings.log_deleted {
        return Ok(());
    }
    let Some(log_channel) = settings.log_channel_id else {
        return Ok(());
    };
    let content = if snapshot.content.is_empty() {
        "*No text content*"
    } else {
        &snapshot.content
    };
    let jump_url = format!("https://discord.com/channels/{guild_id}/{channel_id}/{message_id}");
    let mut embed = serenity::CreateEmbed::new()
        .description(format!(
            "🗑️ **Message deleted in** {} • [Jump to Message]({})\n\n{}",
            channel_id.mention(),
            jump_url,
            content
        ))
        .color(serenity::Colour::DARK_RED)
        .timestamp(serenity::Timestamp::now())
        .author(
            serenity::CreateEmbedAuthor::new(format!(
                "{} ({})",
                snapshot.author_name, snapshot.user_id
            ))
            .icon_url(snapshot.avatar_url),
        );
    if !snapshot.attachment_names.is_empty() {
        embed = embed.field(
            "Attachments (Deleted)",
            truncate(&snapshot.attachment_names.join(", "), 1024),
            false,
        );
    }
    database::remove_snapshot(&data.message_logger_db, channel_id.get(), message_id.get()).await?;
    send_log(ctx, serenity::ChannelId::new(log_channel), embed).await
}

async fn send_log(
    ctx: &serenity::Context,
    channel: serenity::ChannelId,
    embed: serenity::CreateEmbed,
) -> Result<(), Error> {
    channel
        .send_message(ctx, serenity::CreateMessage::new().embed(embed))
        .await?;
    Ok(())
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut output = text
        .chars()
        .take(limit.saturating_sub(3))
        .collect::<String>();
    output.push_str("...");
    output
}

fn add_attachments(
    mut embed: serenity::CreateEmbed,
    attachments: &[serenity::Attachment],
) -> serenity::CreateEmbed {
    if attachments.is_empty() {
        return embed;
    }

    if let Some(image) = attachments.iter().find(|attachment| {
        attachment
            .content_type
            .as_deref()
            .is_some_and(|content_type| content_type.starts_with("image/"))
    }) {
        embed = embed.image(image.url.as_str());
    }

    let mut fields = Vec::new();
    let mut current = String::new();
    for attachment in attachments {
        let link = format!("[{}]({})", attachment.filename, attachment.url);
        let separator = if current.is_empty() { "" } else { "\n" };
        if current.chars().count() + separator.len() + link.chars().count() > 1000
            && !current.is_empty()
        {
            fields.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&link);
    }
    if !current.is_empty() {
        fields.push(current);
    }
    for (index, value) in fields.into_iter().take(20).enumerate() {
        embed = embed.field(
            if index == 0 {
                "Attachments".to_owned()
            } else {
                format!("Attachments ({})", index + 1)
            },
            value,
            false,
        );
    }
    embed
}
