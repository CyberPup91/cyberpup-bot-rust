use crate::{Data, Error};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, ChannelType, CreateActionRow, CreateAllowedMentions, CreateAttachment,
    CreateButton, CreateChannel, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, PermissionOverwrite,
    PermissionOverwriteType, Permissions, RoleId, UserId,
};
use serenity::Mentionable;
use std::time::Duration;

const TICKET_PERMISSIONS: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::READ_MESSAGE_HISTORY)
    .union(Permissions::ATTACH_FILES);

pub struct TicketChannelSpec<'a> {
    pub guild_id: serenity::GuildId,
    pub category_id: u64,
    pub channel_name: &'a str,
    pub opener: UserId,
    pub support_role_id: u64,
    pub additional_member: Option<UserId>,
    pub additional_role: Option<u64>,
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let custom_id = interaction.data.custom_id.as_str();
    if custom_id.starts_with("ticket_panel:create:") {
        create_ticket_from_panel(ctx, data, interaction).await?;
    } else if custom_id == "ticket_control:claim" {
        claim_ticket(ctx, data, interaction).await?;
    } else if custom_id == "ticket_control:close" {
        request_close(ctx, data, interaction).await?;
    } else if let Some(rest) = custom_id.strip_prefix("ticket_control:confirm:") {
        confirm_close(ctx, data, interaction, rest).await?;
    } else if let Some(rest) = custom_id.strip_prefix("ticket_control:cancel:") {
        cancel_close(ctx, interaction, rest).await?;
    }
    Ok(())
}

async fn create_ticket_from_panel(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id else {
        return respond(
            ctx,
            interaction,
            "Tickets can only be opened in a server.",
            true,
        )
        .await;
    };
    let id = interaction
        .data
        .custom_id
        .strip_prefix("ticket_panel:create:")
        .unwrap_or_default();
    let mut parts = id.split(':');
    let Some(panel_guild) = parts.next().and_then(|value| value.parse::<u64>().ok()) else {
        return respond(ctx, interaction, "This ticket panel is invalid.", true).await;
    };
    if panel_guild != guild_id.get() {
        return respond(
            ctx,
            interaction,
            "This panel belongs to a different server.",
            true,
        )
        .await;
    }
    let option_id = parts.next().and_then(|value| value.parse::<i64>().ok());
    if parts.next().is_some() {
        return respond(ctx, interaction, "This ticket panel is invalid.", true).await;
    }

    let Some(config) = super::database::get_config(&data.tickets_db, panel_guild).await? else {
        return respond(ctx, interaction, "❌ Ticketing is not configured.", true).await;
    };
    let (Some(category_id), Some(default_role_id)) = (config.category_id, config.support_role_id)
    else {
        return respond(ctx, interaction, "❌ Ticketing is not configured.", true).await;
    };
    if let (Some(block_role), Some(member)) = (config.block_role_id, interaction.member.as_ref()) {
        if member.roles.iter().any(|role| role.get() == block_role) {
            return respond(
                ctx,
                interaction,
                "🛑 You are restricted from opening tickets.",
                true,
            )
            .await;
        }
    }
    let active = super::database::open_ticket_count(
        &data.tickets_db,
        panel_guild,
        interaction.user.id.get(),
    )
    .await?;
    if active >= config.max_user_tickets.max(1) {
        return respond(
            ctx,
            interaction,
            &format!(
                "❌ You have reached the limit of **{}** open tickets.",
                config.max_user_tickets.max(1)
            ),
            true,
        )
        .await;
    }

    let mut label = "General Support".to_string();
    let mut support_role_id = default_role_id;
    if let Some(option_id) = option_id {
        let Some(option) =
            super::database::get_option(&data.tickets_db, panel_guild, option_id).await?
        else {
            return respond(ctx, interaction, "❌ This ticket category is no longer available. Refresh the panel with `/ticket manage`.", true).await;
        };
        label = option.label;
        support_role_id = option.support_role_id.unwrap_or(default_role_id);
    }

    let Some(guild) = ctx.cache.guild(guild_id).map(|guild| (*guild).clone()) else {
        return respond(
            ctx,
            interaction,
            "❌ I could not load this server. Please try again.",
            true,
        )
        .await;
    };
    let Some(category) = guild.channels.get(&serenity::ChannelId::new(category_id)) else {
        return respond(
            ctx,
            interaction,
            "❌ Configured ticket category was not found.",
            true,
        )
        .await;
    };
    if category.kind != ChannelType::Category {
        return respond(
            ctx,
            interaction,
            "❌ Configured ticket location is not a category.",
            true,
        )
        .await;
    }
    if !guild.roles.contains_key(&RoleId::new(support_role_id)) {
        return respond(
            ctx,
            interaction,
            "❌ Configured support role was not found.",
            true,
        )
        .await;
    }

    interaction.defer(ctx).await?;
    let number = super::database::next_ticket_number(&data.tickets_db, panel_guild).await?;
    let channel_name = ticket_channel_name(&label, number);
    let channel = create_ticket_channel(
        ctx,
        TicketChannelSpec {
            guild_id,
            category_id,
            channel_name: &channel_name,
            opener: interaction.user.id,
            support_role_id,
            additional_member: None,
            additional_role: None,
        },
    )
    .await?;
    if let Err(error) = super::database::add_ticket(
        &data.tickets_db,
        channel.id.get(),
        panel_guild,
        interaction.user.id.get(),
        support_role_id,
    )
    .await
    {
        let _ = channel.delete(ctx).await;
        return Err(error.into());
    }

    let welcome = config
        .ticket_welcome_msg
        .replace("{user}", &interaction.user.id.mention().to_string());
    let embed = ticket_welcome_embed(
        &guild.name,
        number,
        &format!("**Category:** {label}\n\n{welcome}"),
    );
    channel
        .send_message(
            ctx,
            CreateMessage::new()
                .content(format!(
                    "{} | <@&{}>",
                    interaction.user.mention(),
                    support_role_id
                ))
                .allowed_mentions(
                    CreateAllowedMentions::new()
                        .users(vec![interaction.user.id])
                        .roles(vec![RoleId::new(support_role_id)]),
                )
                .embed(embed)
                .components(ticket_controls(None)),
        )
        .await?;
    interaction
        .edit_response(
            ctx,
            EditInteractionResponse::new()
                .content(format!("✅ Ticket opened! Go to {}", channel.mention())),
        )
        .await?;
    Ok(())
}

pub async fn create_ticket_channel(
    ctx: &serenity::Context,
    spec: TicketChannelSpec<'_>,
) -> Result<serenity::GuildChannel, Error> {
    let TicketChannelSpec {
        guild_id,
        category_id,
        channel_name,
        opener,
        support_role_id,
        additional_member,
        additional_role,
    } = spec;
    let everyone = guild_id.everyone_role();
    let mut overwrites = vec![
        PermissionOverwrite {
            allow: Permissions::empty(),
            deny: Permissions::VIEW_CHANNEL,
            kind: PermissionOverwriteType::Role(everyone),
        },
        PermissionOverwrite {
            allow: TICKET_PERMISSIONS,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Member(opener),
        },
        PermissionOverwrite {
            allow: TICKET_PERMISSIONS,
            deny: Permissions::empty(),
            kind: PermissionOverwriteType::Role(RoleId::new(support_role_id)),
        },
    ];
    let bot_id = ctx.cache.current_user().id;
    overwrites.push(PermissionOverwrite {
        allow: TICKET_PERMISSIONS.union(Permissions::MANAGE_CHANNELS),
        deny: Permissions::empty(),
        kind: PermissionOverwriteType::Member(bot_id),
    });
    if let Some(member) = additional_member {
        if member != opener {
            overwrites.push(PermissionOverwrite {
                allow: TICKET_PERMISSIONS,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Member(member),
            });
        }
    }
    if let Some(role) = additional_role {
        if role != support_role_id {
            overwrites.push(PermissionOverwrite {
                allow: TICKET_PERMISSIONS,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Role(RoleId::new(role)),
            });
        }
    }
    Ok(guild_id
        .create_channel(
            ctx,
            CreateChannel::new(channel_name)
                .kind(ChannelType::Text)
                .category(serenity::ChannelId::new(category_id))
                .permissions(overwrites)
                .audit_log_reason("Created a support ticket"),
        )
        .await?)
}

fn ticket_channel_name(label: &str, number: i64) -> String {
    let clean = label
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == ' ' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    let clean = if clean.is_empty() { "support" } else { &clean };
    let suffix = format!("-{number:04}");
    format!(
        "ticket-{}{suffix}",
        clean
            .chars()
            .take(100usize.saturating_sub(7 + suffix.len()))
            .collect::<String>()
    )
}

pub fn ticket_welcome_embed(guild_name: &str, number: i64, description: &str) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("🎫 Ticket #{number:04}"))
        .description(description)
        .color(0x5865F2)
        .timestamp(serenity::Timestamp::now())
        .footer(serenity::CreateEmbedFooter::new(format!(
            "{guild_name} Staff Team"
        )))
}

pub fn ticket_controls(claimed_by: Option<&str>) -> Vec<CreateActionRow> {
    let claim = match claimed_by {
        Some(name) => CreateButton::new("ticket_control:claim")
            .label(format!(
                "Claimed by {}",
                name.chars().take(65).collect::<String>()
            ))
            .style(ButtonStyle::Secondary)
            .disabled(true),
        None => CreateButton::new("ticket_control:claim")
            .label("Claim Ticket")
            .emoji('🙋')
            .style(ButtonStyle::Success),
    };
    vec![CreateActionRow::Buttons(vec![
        claim,
        CreateButton::new("ticket_control:close")
            .label("Close Ticket")
            .emoji('🔒')
            .style(ButtonStyle::Danger),
    ])]
}

async fn claim_ticket(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id else {
        return respond(
            ctx,
            interaction,
            "This action is only available in a server.",
            true,
        )
        .await;
    };
    let Some(ticket) =
        super::database::get_ticket(&data.tickets_db, interaction.channel_id.get()).await?
    else {
        return respond(
            ctx,
            interaction,
            "❌ This is not a tracked active support ticket.",
            true,
        )
        .await;
    };
    if ticket.status != "open" || ticket.guild_id != guild_id.get() {
        return respond(
            ctx,
            interaction,
            "❌ This is not a tracked active support ticket.",
            true,
        )
        .await;
    }
    if ticket.claimed_by.is_some() {
        return respond(
            ctx,
            interaction,
            "❌ This ticket has already been claimed.",
            true,
        )
        .await;
    }
    let config = super::database::get_config(&data.tickets_db, guild_id.get()).await?;
    let support_role_id = ticket
        .support_role_id
        .or_else(|| config.as_ref().and_then(|config| config.support_role_id));
    let is_admin = interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .map(|permissions| permissions.manage_guild())
        .unwrap_or(false);
    let has_support_role = support_role_id.is_some_and(|role_id| {
        interaction
            .member
            .as_ref()
            .is_some_and(|member| member.roles.iter().any(|role| role.get() == role_id))
    });
    if !is_admin && !has_support_role {
        return respond(
            ctx,
            interaction,
            "❌ Only support staff can claim tickets.",
            true,
        )
        .await;
    }
    if !super::database::claim_ticket(
        &data.tickets_db,
        ticket.channel_id,
        interaction.user.id.get(),
    )
    .await?
    {
        return respond(
            ctx,
            interaction,
            "❌ This ticket was just claimed by someone else.",
            true,
        )
        .await;
    }
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .components(ticket_controls(Some(&interaction.user.name))),
            ),
        )
        .await?;
    interaction
        .channel_id
        .send_message(
            ctx,
            CreateMessage::new().content(format!(
                "🛡️ **This ticket has been claimed by {}.** They will be assisting you shortly.",
                interaction.user.mention()
            )),
        )
        .await?;
    Ok(())
}

async fn request_close(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(ticket) =
        super::database::get_ticket(&data.tickets_db, interaction.channel_id.get()).await?
    else {
        return respond(
            ctx,
            interaction,
            "❌ This is not a tracked active support ticket.",
            true,
        )
        .await;
    };
    if !can_close(data, interaction, &ticket).await? {
        return respond(
            ctx,
            interaction,
            "❌ You do not have permission to close this ticket.",
            true,
        )
        .await;
    }
    let requester = interaction.user.id.get();
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("❓ Are you sure you want to close and archive this ticket?")
                    .components(vec![CreateActionRow::Buttons(vec![
                        CreateButton::new(format!(
                            "ticket_control:confirm:{requester}:{}",
                            interaction.channel_id.get()
                        ))
                        .label("Yes, Close")
                        .emoji(serenity::ReactionType::Unicode("⚠️".into()))
                        .style(ButtonStyle::Danger),
                        CreateButton::new(format!(
                            "ticket_control:cancel:{requester}:{}",
                            interaction.channel_id.get()
                        ))
                        .label("Cancel")
                        .style(ButtonStyle::Secondary),
                    ])])
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

async fn can_close(
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
    ticket: &super::database::Ticket,
) -> Result<bool, sqlx::Error> {
    if ticket.status != "open" || Some(ticket.guild_id) != interaction.guild_id.map(|id| id.get()) {
        return Ok(false);
    }
    if ticket.creator_id == interaction.user.id.get() {
        return Ok(true);
    }
    let admin = interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .map(|permissions| permissions.manage_guild())
        .unwrap_or(false);
    let config = super::database::get_config(&data.tickets_db, ticket.guild_id).await?;
    let support_role = ticket
        .support_role_id
        .or_else(|| config.and_then(|config| config.support_role_id));
    let staff = support_role.is_some_and(|role| {
        interaction
            .member
            .as_ref()
            .is_some_and(|member| member.roles.iter().any(|item| item.get() == role))
    });
    Ok(admin || staff)
}

async fn cancel_close(
    ctx: &serenity::Context,
    interaction: &serenity::all::ComponentInteraction,
    rest: &str,
) -> Result<(), Error> {
    if !valid_close_request(interaction, rest) {
        return respond(
            ctx,
            interaction,
            "❌ This close confirmation belongs to another request.",
            true,
        )
        .await;
    }
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .content("❌ Close request cancelled.")
                    .components(vec![]),
            ),
        )
        .await?;
    Ok(())
}

fn valid_close_request(interaction: &serenity::all::ComponentInteraction, rest: &str) -> bool {
    let mut parts = rest.split(':');
    parts.next().and_then(|id| id.parse::<u64>().ok()) == Some(interaction.user.id.get())
        && parts.next().and_then(|id| id.parse::<u64>().ok()) == Some(interaction.channel_id.get())
        && parts.next().is_none()
}

async fn confirm_close(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
    rest: &str,
) -> Result<(), Error> {
    if !valid_close_request(interaction, rest) {
        return respond(
            ctx,
            interaction,
            "❌ This close confirmation belongs to another request.",
            true,
        )
        .await;
    }
    let Some(guild_id) = interaction.guild_id else {
        return respond(
            ctx,
            interaction,
            "This action is only available in a server.",
            true,
        )
        .await;
    };
    let Some(ticket) =
        super::database::get_ticket(&data.tickets_db, interaction.channel_id.get()).await?
    else {
        return respond(ctx, interaction, "❌ Ticket records not found.", true).await;
    };
    if !can_close(data, interaction, &ticket).await? {
        return respond(
            ctx,
            interaction,
            "❌ You no longer have permission to close this ticket.",
            true,
        )
        .await;
    }
    interaction.defer(ctx).await?;
    let transcript = generate_transcript(ctx, interaction.channel_id).await?;
    let config = super::database::get_config(&data.tickets_db, guild_id.get()).await?;
    if let Some(log_channel_id) = config.and_then(|config| config.log_channel_id) {
        let embed = CreateEmbed::new()
            .title("📂 Ticket Archive Generated")
            .color(0x4F545C)
            .timestamp(serenity::Timestamp::now())
            .field(
                "Ticket",
                format!(
                    "#{}",
                    interaction
                        .channel
                        .as_ref()
                        .and_then(|channel| channel.name.as_deref())
                        .unwrap_or("ticket")
                ),
                true,
            )
            .field("Opened By", format!("<@{}>", ticket.creator_id), true)
            .field("Closed By", interaction.user.mention().to_string(), true)
            .field(
                "Claimed By",
                ticket
                    .claimed_by
                    .map(|id| format!("<@{id}>"))
                    .unwrap_or_else(|| "Unclaimed".into()),
                false,
            );
        let filename = format!(
            "transcript-{}.txt",
            interaction
                .channel
                .as_ref()
                .and_then(|channel| channel.name.as_deref())
                .unwrap_or("ticket")
        );
        let attachment = CreateAttachment::bytes(transcript.into_bytes(), filename);
        if let Err(error) = serenity::ChannelId::new(log_channel_id)
            .send_message(ctx, CreateMessage::new().embed(embed).add_file(attachment))
            .await
        {
            tracing::error!(
                "Could not save ticket transcript to channel {log_channel_id}: {error}"
            );
            interaction.edit_response(ctx, EditInteractionResponse::new().content("❌ I could not save the transcript to the configured log channel, so this ticket remains open. Check the log channel permissions and try again.")).await?;
            return Ok(());
        }
    }
    super::database::close_ticket(&data.tickets_db, ticket.channel_id).await?;
    interaction
        .edit_response(
            ctx,
            EditInteractionResponse::new()
                .content("📁 Ticket archived! Channel deleting in 5 seconds...")
                .components(vec![]),
        )
        .await?;
    let channel_id = interaction.channel_id;
    let http = ctx.http.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if let Err(error) = channel_id.delete(&http).await {
            eprintln!("[Tickets] Could not delete archived channel {channel_id}: {error}");
        }
    });
    Ok(())
}

async fn generate_transcript(
    ctx: &serenity::Context,
    channel_id: serenity::ChannelId,
) -> Result<String, Error> {
    let mut messages = Vec::new();
    let mut before = None;
    for _ in 0..5 {
        let mut request = serenity::GetMessages::new().limit(100);
        if let Some(id) = before {
            request = request.before(id);
        }
        let mut batch = channel_id.messages(ctx, request).await?;
        if batch.is_empty() {
            break;
        }
        before = batch.last().map(|message| message.id);
        messages.append(&mut batch);
        if messages.len() >= 500 {
            break;
        }
    }
    messages.sort_by_key(|message| message.timestamp);
    let mut transcript = format!(
        "=== Support Ticket Transcript: #{} ===\nArchived at: {} UTC\n\n",
        channel_id,
        serenity::Timestamp::now()
    );
    for message in messages.iter().take(500) {
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

async fn respond(
    ctx: &serenity::Context,
    interaction: &serenity::all::ComponentInteraction,
    content: &str,
    ephemeral: bool,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(content)
                    .ephemeral(ephemeral),
            ),
        )
        .await?;
    Ok(())
}
