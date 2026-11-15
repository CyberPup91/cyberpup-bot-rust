pub mod database;
pub mod management;

use crate::{Data, Error};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, ChannelType, CreateActionRow, CreateAttachment, CreateButton, CreateEmbed,
    CreateEmbedFooter, CreateMessage, EditChannel, PermissionOverwrite, PermissionOverwriteType,
    Permissions, RoleId,
};
use serenity::Mentionable;

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    database::init_db(pool).await
}

pub async fn channel_created(
    ctx: &serenity::Context,
    data: &Data,
    channel: &serenity::GuildChannel,
) -> Result<(), Error> {
    let Some(config) = database::get_config(&data.warnings_db, channel.guild_id.get()).await?
    else {
        return Ok(());
    };
    let (Some(mute_role), Some(category)) = (config.mute_role_id, config.category_id) else {
        return Ok(());
    };
    if channel.id.get() == category
        || channel
            .parent_id
            .is_some_and(|parent| parent.get() == category)
    {
        return Ok(());
    }
    if !matches!(
        channel.kind,
        ChannelType::Text
            | ChannelType::Voice
            | ChannelType::Stage
            | ChannelType::Forum
            | ChannelType::Category
    ) {
        return Ok(());
    }
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let mut channel = channel.clone();
    apply_mute_overwrite(ctx, &mut channel, RoleId::new(mute_role)).await?;
    Ok(())
}

pub async fn issue_warning(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
    moderator_id: serenity::UserId,
    target: &serenity::Member,
    title: &str,
    explanation: &str,
    require_staff_review: bool,
) -> Result<(), Error> {
    let Some(config) = database::get_config(&data.warnings_db, guild_id.get()).await? else {
        return Err("Warnings system is not configured; use /warnings config first.".into());
    };
    let (Some(category_id), Some(mute_role_id)) = (config.category_id, config.mute_role_id) else {
        return Err("Warnings system is not configured; use /warnings config first.".into());
    };
    let guild = ctx
        .cache
        .guild(guild_id)
        .map(|guild| (*guild).clone())
        .ok_or("Could not load this server.")?;
    let category = guild
        .channels
        .get(&serenity::ChannelId::new(category_id))
        .ok_or("Configured warnings category was not found.")?;
    if category.kind != ChannelType::Category {
        return Err("Configured warnings location is not a category channel.".into());
    }
    let mute_role = guild
        .roles
        .get(&RoleId::new(mute_role_id))
        .ok_or("Configured mute role was not found.")?;
    let bot_id = ctx.cache.current_user().id;
    let bot_member = guild_id.member(ctx, bot_id).await?;
    let bot_top = bot_member
        .roles
        .iter()
        .filter_map(|id| guild.roles.get(id).map(|role| role.position))
        .max()
        .unwrap_or(0);
    if mute_role.position >= bot_top {
        return Err("The mute role is higher than or equal to the bot's highest role.".into());
    }
    if !target.roles.contains(&mute_role.id) {
        target.add_role(ctx, mute_role.id).await?;
    }

    let warning_number = database::next_warning_number(&data.warnings_db, guild_id.get()).await?;
    let clean_name = target
        .user
        .name
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(15)
        .collect::<String>();
    let channel_name = format!(
        "warn-{}-{warning_number:04}",
        if clean_name.is_empty() {
            "user"
        } else {
            &clean_name
        }
    );
    let everyone = guild_id.everyone_role();
    let user_permissions = Permissions::VIEW_CHANNEL
        .union(Permissions::SEND_MESSAGES)
        .union(Permissions::READ_MESSAGE_HISTORY)
        .union(Permissions::ATTACH_FILES)
        .union(Permissions::EMBED_LINKS);
    let mut overwrites = vec![
        PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::VIEW_CHANNEL,
            kind: PermissionOverwriteType::Role(everyone),
        },
        PermissionOverwrite {
            allow: user_permissions,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(target.user.id),
        },
        PermissionOverwrite {
            allow: Permissions::VIEW_CHANNEL
                .union(Permissions::SEND_MESSAGES)
                .union(Permissions::READ_MESSAGE_HISTORY)
                .union(Permissions::MANAGE_CHANNELS),
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(bot_id),
        },
        PermissionOverwrite {
            allow: Permissions::VIEW_CHANNEL
                .union(Permissions::SEND_MESSAGES)
                .union(Permissions::READ_MESSAGE_HISTORY),
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Role(mute_role.id),
        },
    ];
    if let Some(staff_id) = config.staff_role_id {
        overwrites.push(PermissionOverwrite {
            allow: user_permissions,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Role(RoleId::new(staff_id)),
        });
    }
    if moderator_id != target.user.id {
        overwrites.push(PermissionOverwrite {
            allow: user_permissions,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(moderator_id),
        });
    }
    let audit_reason = format!("Warning room for {}", target.user.id);
    let channel = match guild_id
        .create_channel(
            ctx,
            serenity::CreateChannel::new(&channel_name)
                .kind(ChannelType::Text)
                .category(serenity::ChannelId::new(category_id))
                .topic(format!(
                    "Formal Warning for {} ({}) | Issued by {}",
                    target.user.name, target.user.id, moderator_id
                ))
                .permissions(overwrites)
                .audit_log_reason(&audit_reason),
        )
        .await
    {
        Ok(channel) => channel,
        Err(error) => {
            let _ = target.remove_role(ctx, mute_role.id).await;
            return Err(Box::new(error));
        }
    };
    let warning_id = match database::create_warning(
        &data.warnings_db,
        guild_id.get(),
        target.user.id.get(),
        moderator_id.get(),
        channel.id.get(),
        title,
        explanation,
        require_staff_review,
    )
    .await
    {
        Ok(id) => id,
        Err(error) => {
            let _ = channel.delete(ctx).await;
            let _ = target.remove_role(ctx, mute_role.id).await;
            return Err(error.into());
        }
    };

    let instructions = if require_staff_review {
        "🔒 **Staff Review Required:** You are restricted from speaking across the server. Please read the notice and communicate with staff here. Staff must approve lifting the mute."
    } else {
        "⚠️ **Resolution Process:** Read the notice carefully, then acknowledge it below to remove the mute role."
    };
    let embed = CreateEmbed::new()
        .title(format!("⚠️ Formal Warning #{warning_number:04}"))
        .color(0xE74C3C)
        .timestamp(serenity::Timestamp::now())
        .thumbnail(target.user.avatar_url().unwrap_or_default())
        .field(
            "👤 Warned Member",
            format!("{} (`{}`)", target.user.id.mention(), target.user.id),
            true,
        )
        .field(
            "🛡️ Issued By",
            format!("{} (`{}`)", moderator_id.mention(), moderator_id),
            true,
        )
        .field("📋 Rule / Violation", title, false)
        .field("📝 Detailed Notice", explanation, false)
        .field("ℹ️ Instructions", instructions, false)
        .footer(CreateEmbedFooter::new(format!(
            "{} Moderation · Warning ID: {warning_id}",
            guild.name
        )));
    let controls = vec![CreateActionRow::Buttons(vec![
        serenity::CreateButton::new("warning_control:acknowledge")
            .label("I Acknowledge & Accept This Warning")
            .emoji('✅')
            .style(ButtonStyle::Success),
        serenity::CreateButton::new("warning_control:staff_resolve")
            .label("Staff: Resolve & Unmute")
            .emoji('🛡')
            .style(ButtonStyle::Primary),
        serenity::CreateButton::new("warning_control:staff_close")
            .label("Staff: Close & Archive")
            .emoji('📁')
            .style(ButtonStyle::Danger),
    ])];
    channel
        .send_message(
            ctx,
            CreateMessage::new()
                .content(target.user.id.mention().to_string())
                .embed(embed)
                .allowed_mentions(
                    serenity::CreateAllowedMentions::new().users(vec![target.user.id]),
                )
                .components(controls),
        )
        .await?;

    if let Some(log_id) = config.log_channel_id {
        let log = CreateEmbed::new()
            .title("🚨 Formal Warning Issued")
            .color(0xE74C3C)
            .field(
                "Target",
                format!("{} (`{}`)", target.user.id.mention(), target.user.id),
                true,
            )
            .field("Moderator", moderator_id.mention().to_string(), true)
            .field("Channel", channel.mention().to_string(), true)
            .field("Violation", title, false)
            .field(
                "Explanation",
                management::truncate(explanation, 1000),
                false,
            )
            .field(
                "Staff Review Required?",
                if require_staff_review { "Yes" } else { "No" },
                true,
            );
        let _ = serenity::ChannelId::new(log_id)
            .send_message(
                ctx,
                CreateMessage::new()
                    .embed(log)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await;
    }
    Ok(())
}

async fn apply_mute_overwrite(
    ctx: &serenity::Context,
    channel: &mut serenity::GuildChannel,
    role_id: RoleId,
) -> Result<bool, Error> {
    let mut overwrites = channel.permission_overwrites.clone();
    let Some(overwrite) = overwrites
        .iter_mut()
        .find(|overwrite| overwrite.kind == PermissionOverwriteType::Role(role_id))
    else {
        overwrites.push(PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::SEND_MESSAGES
                .union(Permissions::SEND_MESSAGES_IN_THREADS)
                .union(Permissions::CREATE_PUBLIC_THREADS)
                .union(Permissions::CREATE_PRIVATE_THREADS)
                .union(Permissions::SPEAK)
                .union(Permissions::SEND_MESSAGES_IN_THREADS),
            kind: PermissionOverwriteType::Role(role_id),
        });
        channel
            .edit(ctx, EditChannel::new().permissions(overwrites))
            .await?;
        return Ok(true);
    };
    let deny = Permissions::SEND_MESSAGES
        .union(Permissions::SEND_MESSAGES_IN_THREADS)
        .union(Permissions::CREATE_PUBLIC_THREADS)
        .union(Permissions::CREATE_PRIVATE_THREADS)
        .union(Permissions::SPEAK);
    if overwrite.deny.contains(deny) {
        return Ok(false);
    }
    overwrite.allow.remove(deny);
    overwrite.deny |= deny;
    channel
        .edit(ctx, EditChannel::new().permissions(overwrites))
        .await?;
    Ok(true)
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<bool, Error> {
    let id = interaction.data.custom_id.as_str();
    if !id.starts_with("warning_control:") && !id.starts_with("warnings:") {
        return Ok(false);
    }
    match id {
        "warning_control:acknowledge" => acknowledge(ctx, data, interaction).await?,
        "warning_control:staff_resolve" => resolve(ctx, data, interaction).await?,
        "warning_control:staff_close" => prompt_close(ctx, data, interaction).await?,
        "warnings:config" => management::configure_modal(ctx, data, interaction).await?,
        "warnings:sync" => sync_mute_overwrites(ctx, data, interaction).await?,
        _ if id.starts_with("warnings:close_confirm:") => {
            confirm_close(ctx, data, interaction).await?
        }
        "warnings:close_cancel" => respond(ctx, interaction, "Close request cancelled.").await?,
        _ => {
            respond(
                ctx,
                interaction,
                "This warning action is no longer available.",
            )
            .await?
        }
    }
    Ok(true)
}

async fn acknowledge(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(warning) =
        database::active_by_channel(&data.warnings_db, interaction.channel_id.get()).await?
    else {
        return respond(
            ctx,
            interaction,
            "This warning room is not recognized or is already closed.",
        )
        .await;
    };
    if interaction.user.id.get() != warning.user_id {
        return respond(
            ctx,
            interaction,
            "Only the warned user can acknowledge this notice.",
        )
        .await;
    }
    if warning.acknowledged_at.is_some() {
        return respond(
            ctx,
            interaction,
            "You have already acknowledged this warning.",
        )
        .await;
    }
    database::acknowledge(&data.warnings_db, warning.id).await?;
    if warning.require_staff_review {
        return respond(
            ctx,
            interaction,
            "✅ Acknowledged. Staff review is required before the mute can be removed.",
        )
        .await;
    }
    database::resolve(&data.warnings_db, warning.id, interaction.user.id.get()).await?;
    remove_mute_role(ctx, data, warning.guild_id, warning.user_id).await;
    interaction.channel_id.send_message(ctx, CreateMessage::new().embed(
        CreateEmbed::new().title("✅ Warning Acknowledged")
            .description(format!("{} acknowledged the warning and agreed to follow server rules. The mute role has been removed if it was present.", interaction.user.mention()))
            .color(0x2ECC71).timestamp(serenity::Timestamp::now())
    )).await?;
    Ok(())
}

async fn resolve(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return respond(ctx, interaction, "This action is server-only.").await;
    };
    let config = database::get_config(&data.warnings_db, guild_id).await?;
    if !is_staff(interaction.member.as_ref(), config.as_ref()) {
        return respond(
            ctx,
            interaction,
            "Only authorized staff can resolve warnings.",
        )
        .await;
    }
    let Some(warning) =
        database::active_by_channel(&data.warnings_db, interaction.channel_id.get()).await?
    else {
        return respond(ctx, interaction, "Warning not found or already closed.").await;
    };
    database::resolve(&data.warnings_db, warning.id, interaction.user.id.get()).await?;
    remove_mute_role(ctx, data, guild_id, warning.user_id).await;
    interaction
        .channel_id
        .send_message(
            ctx,
            CreateMessage::new().embed(
                CreateEmbed::new()
                    .title("🛡️ Warning Resolved by Staff")
                    .description(format!(
                        "Resolved by {}. The mute role was removed if present.",
                        interaction.user.mention()
                    ))
                    .color(0x2ECC71)
                    .timestamp(serenity::Timestamp::now()),
            ),
        )
        .await?;
    respond(ctx, interaction, "Warning resolved.").await
}

async fn prompt_close(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return respond(ctx, interaction, "This action is server-only.").await;
    };
    let config = database::get_config(&data.warnings_db, guild_id).await?;
    if !is_staff(interaction.member.as_ref(), config.as_ref()) {
        return respond(
            ctx,
            interaction,
            "Only authorized staff can close warning rooms.",
        )
        .await;
    }
    let buttons = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!(
            "warnings:close_confirm:{}:{}",
            interaction.user.id, interaction.channel_id
        ))
        .label("Yes, Archive & Close")
        .style(ButtonStyle::Danger),
        CreateButton::new("warnings:close_cancel")
            .label("Cancel")
            .style(ButtonStyle::Secondary),
    ])];
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content("Archive this warning room and delete its channel?")
                    .components(buttons)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

async fn confirm_close(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let mut parts = interaction.data.custom_id.split(':');
    let _ = parts.next();
    let _ = parts.next();
    let valid_user =
        parts.next().and_then(|id| id.parse::<u64>().ok()) == Some(interaction.user.id.get());
    let valid_channel =
        parts.next().and_then(|id| id.parse::<u64>().ok()) == Some(interaction.channel_id.get());
    if !valid_user || !valid_channel || parts.next().is_some() {
        return respond(
            ctx,
            interaction,
            "This close confirmation belongs to another request.",
        )
        .await;
    }
    let Some(warning) =
        database::active_by_channel(&data.warnings_db, interaction.channel_id.get()).await?
    else {
        return respond(ctx, interaction, "This warning case is no longer active.").await;
    };
    let transcript = generate_transcript(ctx, interaction.channel_id).await?;
    let config = database::get_config(&data.warnings_db, warning.guild_id).await?;
    if let Some(log_channel) = config.and_then(|config| config.log_channel_id) {
        let embed = CreateEmbed::new()
            .title("📂 Warning Room Archived")
            .color(0x4F545C)
            .field(
                "Room",
                format!(
                    "#{}",
                    interaction
                        .channel
                        .as_ref()
                        .and_then(|channel| channel.name.as_deref())
                        .unwrap_or("warning")
                ),
                true,
            )
            .field("Target", format!("<@{}>", warning.user_id), true)
            .field("Closed By", interaction.user.mention().to_string(), true)
            .field("Original Violation", warning.title, false);
        let filename = format!("warning-transcript-{}.txt", interaction.channel_id);
        let attachment = CreateAttachment::bytes(transcript.into_bytes(), filename);
        serenity::ChannelId::new(log_channel)
            .send_message(ctx, CreateMessage::new().embed(embed).add_file(attachment))
            .await?;
    }
    database::close(&data.warnings_db, interaction.channel_id.get()).await?;
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content("📁 Warning room archived. Channel deleting in 5 seconds.")
                    .ephemeral(true),
            ),
        )
        .await?;
    let http = ctx.http.clone();
    let channel = interaction.channel_id;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let _ = channel.delete(&http).await;
    });
    Ok(())
}

fn is_staff(
    member: Option<&serenity::all::Member>,
    config: Option<&database::WarningConfig>,
) -> bool {
    let Some(member) = member else { return false };
    if member
        .permissions
        .is_some_and(|permissions| permissions.administrator() || permissions.manage_guild())
    {
        return true;
    }
    config
        .and_then(|config| config.staff_role_id)
        .is_some_and(|role| member.roles.iter().any(|id| id.get() == role))
}

async fn remove_mute_role(ctx: &serenity::Context, data: &Data, guild_id: u64, user_id: u64) {
    let config = database::get_config(&data.warnings_db, guild_id)
        .await
        .ok()
        .flatten();
    let Some(role_id) = config.and_then(|config| config.mute_role_id) else {
        return;
    };
    if let Ok(member) = serenity::GuildId::new(guild_id)
        .member(ctx, serenity::UserId::new(user_id))
        .await
    {
        let _ = member.remove_role(ctx, RoleId::new(role_id)).await;
    }
}

async fn generate_transcript(
    ctx: &serenity::Context,
    channel: serenity::ChannelId,
) -> Result<String, Error> {
    let mut messages = Vec::new();
    let mut before = None;
    for _ in 0..10 {
        let mut request = serenity::GetMessages::new().limit(100);
        if let Some(id) = before {
            request = request.before(id);
        }
        let mut batch = channel.messages(ctx, request).await?;
        if batch.is_empty() {
            break;
        }
        before = batch.last().map(|message| message.id);
        messages.append(&mut batch);
        if messages.len() >= 1000 {
            break;
        }
    }
    messages.sort_by_key(|message| message.timestamp);
    let mut transcript = format!(
        "=== Warning Room Transcript: #{channel} ===\nArchived at: {} UTC\n\n",
        serenity::Timestamp::now()
    );
    for message in messages.iter().take(1000) {
        transcript.push_str(&format!(
            "[{}] {} ({}): {}",
            message.timestamp, message.author.name, message.author.id, message.content
        ));
        if !message.attachments.is_empty() {
            transcript.push_str(" [Attachments: ");
            transcript.push_str(
                &message
                    .attachments
                    .iter()
                    .map(|attachment| attachment.url.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            transcript.push(']');
        }
        transcript.push('\n');
    }
    Ok(transcript)
}

async fn sync_mute_overwrites(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id else {
        return respond(ctx, interaction, "This action is server-only.").await;
    };
    let Some(config) = database::get_config(&data.warnings_db, guild_id.get()).await? else {
        return respond(ctx, interaction, "Configure a mute role first.").await;
    };
    let Some(role_id) = config.mute_role_id else {
        return respond(ctx, interaction, "Configure a mute role first.").await;
    };
    let Some(channels) = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.channels.values().cloned().collect::<Vec<_>>())
    else {
        return respond(ctx, interaction, "I could not load this server.").await;
    };
    interaction.defer(ctx).await?;
    let mut updated = 0;
    let mut skipped = 0;
    let mut failed = 0;
    for mut channel in channels {
        if channel.id.get() == config.category_id.unwrap_or_default()
            || channel
                .parent_id
                .is_some_and(|id| Some(id.get()) == config.category_id)
        {
            skipped += 1;
            continue;
        }
        match apply_mute_overwrite(ctx, &mut channel, RoleId::new(role_id)).await {
            Ok(true) => {
                updated += 1;
                tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            }
            Ok(false) => skipped += 1,
            Err(_) => failed += 1,
        }
    }
    interaction.edit_response(ctx, serenity::EditInteractionResponse::new().content(format!("Mute overwrite sync complete. Updated: {updated}; already protected/skipped: {skipped}; failed: {failed}."))).await?;
    Ok(())
}

pub async fn sync_overwrites(
    ctx: &serenity::Context,
    data: &Data,
    guild_id: serenity::GuildId,
) -> Result<(usize, usize, usize), Error> {
    let Some(config) = database::get_config(&data.warnings_db, guild_id.get()).await? else {
        return Err("Warnings settings are not configured.".into());
    };
    let Some(role_id) = config.mute_role_id else {
        return Err("Configure a mute role first.".into());
    };
    let Some(channels) = ctx
        .cache
        .guild(guild_id)
        .map(|guild| guild.channels.values().cloned().collect::<Vec<_>>())
    else {
        return Err("Could not load this server.".into());
    };
    let mut updated = 0;
    let mut skipped = 0;
    let mut failed = 0;
    for mut channel in channels {
        if channel.id.get() == config.category_id.unwrap_or_default()
            || channel
                .parent_id
                .is_some_and(|id| Some(id.get()) == config.category_id)
        {
            skipped += 1;
            continue;
        }
        match apply_mute_overwrite(ctx, &mut channel, RoleId::new(role_id)).await {
            Ok(true) => {
                updated += 1;
                tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            }
            Ok(false) => skipped += 1,
            Err(_) => failed += 1,
        }
    }
    Ok((updated, skipped, failed))
}

async fn respond(
    ctx: &serenity::Context,
    interaction: &serenity::all::ComponentInteraction,
    message: &str,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx,
            serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(message)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}
