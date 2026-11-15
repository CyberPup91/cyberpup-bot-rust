use crate::{Context, Error};
use futures::StreamExt;
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInputText, InputTextStyle,
};
use serenity::collector::ComponentInteractionCollector;
use serenity::utils::CreateQuickModal;
use sqlx::Row;
use std::time::Duration;

pub async fn manage(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = ctx.guild_id().map(|id| id.get()) else {
        ctx.say("Use this command in a server.").await?;
        return Ok(());
    };
    sqlx::query("INSERT OR IGNORE INTO vent_config(guild_id) VALUES(?)")
        .bind(guild_id as i64)
        .execute(&ctx.data().vents_db)
        .await?;
    let reply = ctx
        .send(
            poise::CreateReply::default()
                .embed(dashboard(ctx, guild_id).await?)
                .components(controls())
                .ephemeral(true),
        )
        .await?;
    let message = reply.message().await?;
    let collector = ComponentInteractionCollector::new(&ctx.serenity_context().shard)
        .author_id(ctx.author().id)
        .channel_id(ctx.channel_id())
        .message_id(message.id)
        .filter(|i| i.data.custom_id.starts_with("vents:manage:"))
        .timeout(Duration::from_secs(300));
    let mut stream = Box::pin(collector.stream());
    while let Some(i) = stream.next().await {
        let action = i
            .data
            .custom_id
            .strip_prefix("vents:manage:")
            .unwrap_or_default();
        let res = match action {
            "channels" => configure_channels(ctx, &i, guild_id).await,
            "panel" => customize_panel(ctx, &i, guild_id).await,
            "deploy" => {
                i.defer_ephemeral(ctx.serenity_context()).await?;
                let row = sqlx::query("SELECT vent_channel_id FROM vent_config WHERE guild_id=?")
                    .bind(guild_id as i64)
                    .fetch_one(&ctx.data().vents_db)
                    .await?;
                if let Some(channel) = row.try_get::<Option<i64>, _>("vent_channel_id")? {
                    super::stick_panel(
                        ctx.serenity_context(),
                        ctx.data(),
                        serenity::GuildId::new(guild_id),
                        serenity::ChannelId::new(channel as u64),
                        true,
                    )
                    .await?;
                    i.edit_response(
                        ctx.serenity_context(),
                        serenity::EditInteractionResponse::new().content(
                            "✅ Panel deployed. It will move to the bottom as messages arrive.",
                        ),
                    )
                    .await?;
                    Ok(())
                } else {
                    i.edit_response(
                        ctx.serenity_context(),
                        serenity::EditInteractionResponse::new()
                            .content("Configure the vent channel first."),
                    )
                    .await?;
                    Ok(())
                }
            }
            "unban" => unban_modal(ctx, &i, guild_id).await,
            _ => Ok(()),
        };
        if let Err(e) = res {
            tracing::warn!("Vents dashboard action failed: {e}");
        }
        if let Ok(embed) = dashboard(ctx, guild_id).await {
            let _ = ctx
                .channel_id()
                .edit_message(
                    ctx.serenity_context(),
                    message.id,
                    serenity::EditMessage::new()
                        .embed(embed)
                        .components(controls()),
                )
                .await;
        }
    }
    Ok(())
}

fn controls() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("vents:manage:channels")
            .label("Configure Channels")
            .emoji('⚙')
            .style(ButtonStyle::Primary),
        CreateButton::new("vents:manage:panel")
            .label("Customize Panel")
            .emoji('🎨')
            .style(ButtonStyle::Secondary),
        CreateButton::new("vents:manage:deploy")
            .label("Deploy / Refresh Panel")
            .emoji('🚀')
            .style(ButtonStyle::Success),
        CreateButton::new("vents:manage:unban")
            .label("Unban User")
            .emoji('🔓')
            .style(ButtonStyle::Secondary),
    ])]
}

async fn dashboard(ctx: Context<'_>, guild: u64) -> Result<CreateEmbed, Error> {
    let row = sqlx::query("SELECT * FROM vent_config WHERE guild_id=?")
        .bind(guild as i64)
        .fetch_optional(&ctx.data().vents_db)
        .await?;
    let (vent, logs, cooldown, title, footer) = if let Some(row) = row {
        (
            row.try_get::<Option<i64>, _>("vent_channel_id")?,
            row.try_get::<Option<i64>, _>("log_channel_id")?,
            row.try_get::<i64, _>("cooldown_seconds")?,
            row.try_get::<Option<String>, _>("panel_title")?,
            row.try_get::<Option<String>, _>("panel_footer")?,
        )
    } else {
        (None, None, 300, None, None)
    };
    let bans: i64 = sqlx::query_scalar("SELECT count(*) FROM vent_bans WHERE guild_id=?")
        .bind(guild as i64)
        .fetch_one(&ctx.data().vents_db)
        .await?;
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM vents WHERE guild_id=? AND status='active'")
            .bind(guild as i64)
            .fetch_one(&ctx.data().vents_db)
            .await?;
    Ok(CreateEmbed::new()
        .title("📝 Vent System Dashboard")
        .description(
            "Configure anonymous venting, customize the sticky panel, and manage vents bans.",
        )
        .color(0x800080)
        .field(
            "Channels & Cooldown",
            format!(
                "**Vent:** {}\n**Logs:** {}\n**Cooldown:** {}",
                vent.map(|x| format!("<#{}>", x))
                    .unwrap_or_else(|| "Not configured".into()),
                logs.map(|x| format!("<#{}>", x))
                    .unwrap_or_else(|| "Not configured".into()),
                if cooldown == 0 {
                    "Disabled".into()
                } else {
                    format!("{cooldown}s")
                }
            ),
            false,
        )
        .field(
            "Panel Embed",
            format!(
                "**Title:** {}\n**Footer:** {}",
                title.unwrap_or_else(|| "📝 Anonymous Vents".into()),
                footer.unwrap_or_else(|| "Be kind. Abuse will result in a vents ban.".into())
            ),
            false,
        )
        .field(
            "Stats",
            format!("Active vents: **{active}**\nBanned users: **{bans}**"),
            true,
        ))
}

async fn configure_channels(
    ctx: Context<'_>,
    i: &serenity::ComponentInteraction,
    guild: u64,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT vent_channel_id,log_channel_id,cooldown_seconds,panel_message_id FROM vent_config WHERE guild_id=?",
    )
    .bind(guild as i64)
    .fetch_one(&ctx.data().vents_db)
    .await?;
    let channel = |name: &str| {
        row.try_get::<Option<i64>, _>(name)
            .ok()
            .flatten()
            .map(|x| x.to_string())
            .unwrap_or_default()
    };
    let modal = CreateQuickModal::new("Configure Vent System")
        .timeout(Duration::from_secs(300))
        .field(
            CreateInputText::new(InputTextStyle::Short, "Vent Channel ID", "Channel ID")
                .value(channel("vent_channel_id")),
        )
        .field(
            CreateInputText::new(InputTextStyle::Short, "Staff Log Channel ID", "Channel ID")
                .value(channel("log_channel_id")),
        )
        .field(
            CreateInputText::new(
                InputTextStyle::Short,
                "Cooldown seconds (0 disables)",
                "300",
            )
            .value(row.try_get::<i64, _>("cooldown_seconds")?.to_string()),
        );
    if let Some(response) = i.quick_modal(ctx.serenity_context(), modal).await? {
        if response.inputs.len() < 3 {
            return Ok(());
        }
        let parse = |v: &str| v.trim().parse::<u64>();
        let (Ok(vent), Ok(log), Ok(cd)) = (
            parse(&response.inputs[0]),
            parse(&response.inputs[1]),
            response.inputs[2].trim().parse::<i64>(),
        ) else {
            response.interaction.create_response(ctx.serenity_context(),serenity::CreateInteractionResponse::Message(serenity::CreateInteractionResponseMessage::new().content("Channel IDs and cooldown must be valid numbers; cooldown cannot be negative.").ephemeral(true))).await?;
            return Ok(());
        };
        if cd < 0 {
            response
                .interaction
                .create_response(
                    ctx.serenity_context(),
                    serenity::CreateInteractionResponse::Message(
                        serenity::CreateInteractionResponseMessage::new()
                            .content("Cooldown must be non-negative.")
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        }
        let old_channel = row.try_get::<Option<i64>, _>("vent_channel_id")?;
        let old_panel = row.try_get::<Option<i64>, _>("panel_message_id")?;
        if old_channel.is_some_and(|id| id as u64 != vent) {
            if let (Some(channel), Some(panel)) = (old_channel, old_panel) {
                let _ = serenity::ChannelId::new(channel as u64)
                    .delete_message(
                        ctx.serenity_context(),
                        serenity::MessageId::new(panel as u64),
                    )
                    .await;
            }
        }
        sqlx::query("UPDATE vent_config SET panel_message_id=CASE WHEN vent_channel_id<>? THEN NULL ELSE panel_message_id END,vent_channel_id=?,log_channel_id=?,cooldown_seconds=? WHERE guild_id=?").bind(vent as i64).bind(vent as i64).bind(log as i64).bind(cd).bind(guild as i64).execute(&ctx.data().vents_db).await?;
        response
            .interaction
            .create_response(
                ctx.serenity_context(),
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("✅ Vent channels and cooldown saved.")
                        .ephemeral(true),
                ),
            )
            .await?;
    }
    Ok(())
}

async fn customize_panel(
    ctx: Context<'_>,
    i: &serenity::ComponentInteraction,
    guild: u64,
) -> Result<(), Error> {
    let row = sqlx::query(
        "SELECT panel_title,panel_description,panel_footer FROM vent_config WHERE guild_id=?",
    )
    .bind(guild as i64)
    .fetch_one(&ctx.data().vents_db)
    .await?;
    let field = |name: &str, default: &str| {
        row.try_get::<Option<String>, _>(name)
            .ok()
            .flatten()
            .unwrap_or_else(|| default.to_owned())
    };
    let modal=CreateQuickModal::new("Customize Vent Panel Embed").timeout(Duration::from_secs(300))
        .field(CreateInputText::new(InputTextStyle::Short,"Embed Title","Panel title").value(field("panel_title","📝 Anonymous Vents")))
        .field(CreateInputText::new(InputTextStyle::Paragraph,"Embed Description","Instructions shown on the panel").value(field("panel_description","Need to get something off your chest?\n\nClick the button below to submit an **anonymous** vent. Your identity is never shown publicly — only staff can see it in private logs for safety.\n\nReplies are also anonymous and appear in a thread under each vent.")))
        .field(CreateInputText::new(InputTextStyle::Short,"Embed Footer","Footer text").value(field("panel_footer","Be kind. Abuse will result in a vents ban.")));
    if let Some(response) = i.quick_modal(ctx.serenity_context(), modal).await? {
        if response.inputs.len() >= 3 {
            sqlx::query("UPDATE vent_config SET panel_title=?,panel_description=?,panel_footer=? WHERE guild_id=?").bind(&response.inputs[0]).bind(&response.inputs[1]).bind(&response.inputs[2]).bind(guild as i64).execute(&ctx.data().vents_db).await?;
            let row = sqlx::query("SELECT vent_channel_id FROM vent_config WHERE guild_id=?")
                .bind(guild as i64)
                .fetch_one(&ctx.data().vents_db)
                .await?;
            if let Some(channel) = row.try_get::<Option<i64>, _>("vent_channel_id")? {
                let panel_ctx = ctx.serenity_context().clone();
                let panel_data = ctx.data().clone();
                tokio::spawn(async move {
                    let _ = super::stick_panel(
                        &panel_ctx,
                        &panel_data,
                        serenity::GuildId::new(guild),
                        serenity::ChannelId::new(channel as u64),
                        true,
                    )
                    .await;
                });
            }
            response
                .interaction
                .create_response(
                    ctx.serenity_context(),
                    serenity::CreateInteractionResponse::Message(
                        serenity::CreateInteractionResponseMessage::new()
                            .content("✅ Vent panel embed saved. Refresh queued.")
                            .ephemeral(true),
                    ),
                )
                .await?;
        }
    }
    Ok(())
}

async fn unban_modal(
    ctx: Context<'_>,
    i: &serenity::ComponentInteraction,
    guild: u64,
) -> Result<(), Error> {
    let modal = CreateQuickModal::new("Unban from Vents System")
        .timeout(Duration::from_secs(300))
        .short_field("Discord User ID");
    if let Some(response) = i.quick_modal(ctx.serenity_context(), modal).await? {
        let Ok(uid) = response
            .inputs
            .first()
            .unwrap_or(&String::new())
            .trim()
            .parse::<u64>()
        else {
            response
                .interaction
                .create_response(
                    ctx.serenity_context(),
                    serenity::CreateInteractionResponse::Message(
                        serenity::CreateInteractionResponseMessage::new()
                            .content("User ID must be numeric.")
                            .ephemeral(true),
                    ),
                )
                .await?;
            return Ok(());
        };
        let result = sqlx::query("DELETE FROM vent_bans WHERE guild_id=? AND user_id=?")
            .bind(guild as i64)
            .bind(uid as i64)
            .execute(&ctx.data().vents_db)
            .await?;
        let text = if result.rows_affected() > 0 {
            format!("✅ User `{uid}` has been unbanned from vents.")
        } else {
            "That user is not currently banned from vents.".to_owned()
        };
        response
            .interaction
            .create_response(
                ctx.serenity_context(),
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .content(text)
                        .ephemeral(true),
                ),
            )
            .await?;
    }
    Ok(())
}
