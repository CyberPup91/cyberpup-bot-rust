use crate::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, ChannelType, CreateActionRow, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, InputTextStyle, RoleId, UserId,
};
use serenity::utils::CreateQuickModal;
use serenity::Mentionable;
use std::time::Duration;

#[derive(poise::Modal)]
#[name = "Issue Formal Warning"]
struct IssueWarningModal {
    #[name = "Violation Title / Rule(s) Broken"]
    violation: String,
    #[name = "Detailed Explanation / Message to User"]
    #[paragraph]
    explanation: String,
}

/// Warning system administration, history, and case channels.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("config", "sync_mute_overwrites", "history")
)]
pub async fn warnings(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Issue a formal warning and create a private warning room.
#[poise::command(slash_command, guild_only, required_permissions = "MODERATE_MEMBERS")]
pub async fn warn(
    ctx: Context<'_>,
    #[description = "Member to warn"] target: UserId,
    #[description = "Require staff approval before lifting the mute"] require_staff_review: Option<
        bool,
    >,
) -> Result<(), Error> {
    if target == ctx.author().id {
        ctx.send(
            poise::CreateReply::default()
                .content("❌ You cannot warn yourself.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let guild_id = ctx.guild_id().ok_or("This command requires a server.")?;

    let member = match guild_id.member(ctx, target).await {
        Ok(member) => member,
        Err(_) => {
            ctx.send(
                poise::CreateReply::default()
                    .content("❌ The target must be a member of this server.")
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    };

    if member.user.bot {
        ctx.send(
            poise::CreateReply::default()
                .content("❌ You cannot issue warnings to bot accounts.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    if !can_warn(ctx, guild_id, &member) {
        ctx.send(
            poise::CreateReply::default()
                .content("❌ You cannot warn a member with a role equal to or higher than yours.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }

    let poise::Context::Application(app_ctx) = ctx else {
        return Ok(());
    };

    let Some(form) = poise::execute_modal::<_, _, IssueWarningModal>(
        app_ctx,
        None,
        Some(Duration::from_secs(300)),
    )
    .await?
    else {
        return Ok(());
    };

    let violation = form.violation.trim();
    let explanation = form.explanation.trim();

    if violation.is_empty() || explanation.is_empty() {
        app_ctx
            .send(
                poise::CreateReply::default()
                    .content("❌ The violation title and explanation are required.")
                    .ephemeral(true),
            )
            .await?;
        return Ok(());
    }

    super::issue_warning(
        app_ctx.serenity_context(),
        app_ctx.data(),
        guild_id,
        app_ctx.author().id,
        &member,
        violation,
        explanation,
        require_staff_review.unwrap_or(false),
    )
    .await?;

    app_ctx
        .send(
            poise::CreateReply::default()
                .content(format!("✅ Warning issued to {}.", target.mention()))
                .ephemeral(true),
        )
        .await?;

    Ok(())
}

/// Configure warning rooms, roles, and transcript routing.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_GUILD")]
pub async fn config(
    ctx: Context<'_>,
    #[description = "Category for private warning rooms"] category: Option<serenity::ChannelId>,
    #[description = "Role assigned to warned members"] mute_role: Option<RoleId>,
    #[description = "Staff role allowed to manage warning rooms"] staff_role: Option<RoleId>,
    #[description = "Channel for logs and transcripts"] log_channel: Option<serenity::ChannelId>,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command requires a server.")?
        .get();
    if let Some(category) = category {
        let is_category = ctx
            .serenity_context()
            .cache
            .guild(serenity::GuildId::new(guild_id))
            .and_then(|guild| {
                guild
                    .channels
                    .get(&category)
                    .map(|channel| channel.kind == ChannelType::Category)
            })
            .unwrap_or(false);
        if !is_category {
            ctx.say("❌ The selected channel must be a category.")
                .await?;
            return Ok(());
        }
    }
    if category.is_some() || mute_role.is_some() || staff_role.is_some() || log_channel.is_some() {
        super::database::set_config(
            &ctx.data().warnings_db,
            guild_id,
            category.map(|id| id.get()),
            staff_role.map(|id| id.get()),
            mute_role.map(|id| id.get()),
            log_channel.map(|id| id.get()),
        )
        .await?;
    }
    let embed = dashboard_embed(
        ctx.data(),
        guild_id,
        &ctx.guild()
            .map(|guild| guild.name.clone())
            .unwrap_or_else(|| "Server".into()),
    )
    .await?;
    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .components(dashboard_components())
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Apply mute restrictions to existing channels and categories.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    rename = "sync-mute-overwrites"
)]
pub async fn sync_mute_overwrites(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("This command requires a server.")?;
    ctx.defer_ephemeral().await?;
    let (updated, skipped, failed) =
        super::sync_overwrites(ctx.serenity_context(), ctx.data(), guild_id).await?;
    ctx.say(format!("Mute overwrite sync complete. Updated: {updated}; already protected or skipped: {skipped}; failed: {failed}.")).await?;
    Ok(())
}

/// View recent warning records for a user.
#[poise::command(slash_command, guild_only, required_permissions = "MODERATE_MEMBERS")]
pub async fn history(
    ctx: Context<'_>,
    #[description = "User whose warning history to review"] user: UserId,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command requires a server.")?
        .get();

    let records = super::database::history(&ctx.data().warnings_db, guild_id, user.get()).await?;

    if records.is_empty() {
        ctx.say(format!("ℹ️ No warnings recorded for {}.", user.mention()))
            .await?;
        return Ok(());
    }

    let mut embed = CreateEmbed::new()
        .title(format!("📜 Warning History: {user}"))
        .description(format!("Showing {} most recent warning(s).", records.len()))
        .color(0xF1C40F);

    for record in records {
        let date = chrono::DateTime::parse_from_rfc3339(&record.created_at)
            .map(|time| format!("<t:{}:f>", time.timestamp()))
            .unwrap_or_else(|_| record.created_at.clone());

        let status = match record.status.as_str() {
            "resolved" => "🟢 Resolved",
            "closed" => "📁 Closed",
            _ => "🔴 Active",
        };

        let channel = record
            .channel_id
            .map(|id| format!("<#{id}>"))
            .unwrap_or_else(|| "Unknown".into());

        let resolution = match (record.resolved_by, record.resolved_at.as_deref()) {
            (Some(moderator_id), Some(timestamp)) => {
                let resolved_date = chrono::DateTime::parse_from_rfc3339(timestamp)
                    .map(|time| format!("<t:{}:f>", time.timestamp()))
                    .unwrap_or_else(|_| timestamp.to_string());

                format!("\n**Resolved by:** <@{moderator_id}>\n**Resolved:** {resolved_date}")
            }
            (Some(moderator_id), None) => {
                format!("\n**Resolved by:** <@{moderator_id}>")
            }
            (None, Some(timestamp)) => {
                let resolved_date = chrono::DateTime::parse_from_rfc3339(timestamp)
                    .map(|time| format!("<t:{}:f>", time.timestamp()))
                    .unwrap_or_else(|_| timestamp.to_string());

                format!("\n**Resolved:** {resolved_date}")
            }
            (None, None) => String::new(),
        };

        embed = embed.field(
            format!("Warning #{} — {}", record.id, record.title),
            format!(
                "**Date:** {date}\n\
                 **Moderator:** <@{}>\n\
                 **Channel:** {channel}\n\
                 **Status:** {}{}\n\
                 **Explanation:** {}{}",
                record.moderator_id,
                status,
                if record.acknowledged_at.is_some() {
                    " (Acknowledged)"
                } else {
                    ""
                },
                truncate(&record.explanation, 180),
                resolution,
            ),
            false,
        );
    }

    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
        .await?;

    Ok(())
}

fn can_warn(ctx: Context<'_>, guild_id: serenity::GuildId, target: &serenity::Member) -> bool {
    let actor_id = ctx.author().id;
    let Some(guild) = ctx.serenity_context().cache.guild(guild_id) else {
        return false;
    };
    if actor_id == guild.owner_id {
        return true;
    }
    let Some(actor) = guild.members.get(&actor_id) else {
        return false;
    };
    let highest = |member: &serenity::Member| {
        member
            .roles
            .iter()
            .filter_map(|id| guild.roles.get(id).map(|role| role.position))
            .max()
            .unwrap_or(0)
    };
    highest(actor) > highest(target)
}

fn dashboard_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("warnings:config")
            .label("⚙️ Configure Settings")
            .style(ButtonStyle::Primary),
        CreateButton::new("warnings:sync")
            .label("🔄 Sync Mute Overwrites")
            .style(ButtonStyle::Secondary),
    ])]
}

async fn dashboard_embed(
    data: &Data,
    guild_id: u64,
    guild_name: &str,
) -> Result<CreateEmbed, Error> {
    let config = super::database::get_config(&data.warnings_db, guild_id).await?;
    let mut embed = CreateEmbed::new()
        .title(format!("⚠️ Warnings Control Dashboard: {guild_name}"))
        .description(
            "Manage warning routing, mute roles, transcript logs, and channel restrictions.",
        )
        .color(0x5865F2)
        .timestamp(serenity::Timestamp::now());
    if let Some(config) = config {
        embed = embed
            .field(
                "📂 Warnings Category",
                config
                    .category_id
                    .map(|id| format!("<#{id}>"))
                    .unwrap_or_else(|| "Not set".into()),
                true,
            )
            .field(
                "🔇 Mute Role",
                config
                    .mute_role_id
                    .map(|id| format!("<@&{id}>"))
                    .unwrap_or_else(|| "Not set".into()),
                true,
            )
            .field(
                "🛡️ Staff Role",
                config
                    .staff_role_id
                    .map(|id| format!("<@&{id}>"))
                    .unwrap_or_else(|| "Not set (admins only)".into()),
                true,
            )
            .field(
                "📜 Log Channel",
                config
                    .log_channel_id
                    .map(|id| format!("<#{id}>"))
                    .unwrap_or_else(|| "Not set".into()),
                true,
            )
            .field("📊 Warning Count", config.warning_count.to_string(), true);
    } else {
        embed = embed.field(
            "Setup required",
            "Use Configure Settings to choose a category and mute role.",
            false,
        );
    }
    Ok(embed)
}

pub async fn configure_modal(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return Ok(());
    };
    let current = super::database::get_config(&data.warnings_db, guild_id).await?;
    let modal = CreateQuickModal::new("Configure Warnings Routing")
        .timeout(Duration::from_secs(300))
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Warnings Category ID",
                "Category channel ID",
            )
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.category_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            ),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Mute Role ID",
                "Role ID assigned to warned users",
            )
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.mute_role_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            ),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Staff Role ID (Optional)",
                "Blank leaves the current value",
            )
            .required(false)
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.staff_role_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            ),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Log Channel ID (Optional)",
                "Blank leaves the current value",
            )
            .required(false)
            .value(
                current
                    .as_ref()
                    .and_then(|c| c.log_channel_id)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            ),
        );
    if let Some(response) = interaction.quick_modal(ctx, modal).await? {
        if response.inputs.len() < 4 {
            return Ok(());
        }
        let parse = |value: &str| value.trim().parse::<u64>().ok();
        let (Some(category), Some(mute_role)) =
            (parse(&response.inputs[0]), parse(&response.inputs[1]))
        else {
            response
                .interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("Category and mute role must be numeric IDs.")
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        };
        super::database::set_config(
            &data.warnings_db,
            guild_id,
            Some(category),
            parse(&response.inputs[2]),
            Some(mute_role),
            parse(&response.inputs[3]),
        )
        .await?;
        response
            .interaction
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("✅ Warning settings saved.")
                        .ephemeral(true),
                ),
            )
            .await?;
    }
    Ok(())
}

pub fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    format!(
        "{}…",
        value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>()
    )
}
