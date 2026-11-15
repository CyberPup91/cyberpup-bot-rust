use crate::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use serenity::all::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, InputTextStyle, RoleId,
};
use serenity::utils::CreateQuickModal;
use serenity::Mentionable;

/// Configure the counting game and view its leaderboards.
#[poise::command(
    slash_command,
    guild_only,
    subcommands("dashboard", "set_role", "set_leaderboard_channel", "leaderboard")
)]
pub async fn counting(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Open the private counting configuration dashboard.
#[poise::command(slash_command, guild_only, required_permissions = "ADMINISTRATOR")]
pub async fn dashboard(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command requires a server.")?
        .get();
    let embed = dashboard_embed(ctx.data(), guild_id).await?;
    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .components(dashboard_components())
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

/// Choose the role given to the most recent successful counter.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "ADMINISTRATOR",
    rename = "setrole"
)]
pub async fn set_role(
    ctx: Context<'_>,
    #[description = "Role awarded to the latest successful counter"] role: Option<RoleId>,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command requires a server.")?
        .get();
    super::database::get_config(&ctx.data().counting_db, guild_id).await?;
    sqlx::query("UPDATE guild_config SET role_id = ? WHERE guild_id = ?")
        .bind(role.map(|id| id.get() as i64))
        .bind(guild_id as i64)
        .execute(&ctx.data().counting_db)
        .await?;
    let result = role
        .map(|id| format!("Counting role set to {}.", id.mention()))
        .unwrap_or_else(|| "Counting role removed.".to_string());
    ctx.say(result).await?;
    Ok(())
}

/// Configure or disable the persistent leaderboard channel.
#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "ADMINISTRATOR",
    rename = "setleaderboardchannel"
)]
pub async fn set_leaderboard_channel(
    ctx: Context<'_>,
    #[description = "Channel for the persistent leaderboard; leave empty to disable"]
    channel: Option<serenity::ChannelId>,
) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command requires a server.")?
        .get();
    super::database::get_config(&ctx.data().counting_db, guild_id).await?;
    sqlx::query("UPDATE guild_config SET leaderboard_channel_id = ?, leaderboard_message_id = NULL WHERE guild_id = ?")
        .bind(channel.map(|id| id.get() as i64))
        .bind(guild_id as i64)
        .execute(&ctx.data().counting_db)
        .await?;
    if let Some(channel) = channel {
        super::update_leaderboard(ctx.serenity_context(), ctx.data(), guild_id).await?;
        ctx.say(format!(
            "Persistent leaderboard channel set to {}.",
            channel.mention()
        ))
        .await?;
    } else {
        ctx.say("Persistent leaderboard disabled.").await?;
    }
    Ok(())
}

/// Show the top counting balances.
#[poise::command(slash_command, guild_only)]
pub async fn leaderboard(ctx: Context<'_>) -> Result<(), Error> {
    let guild_id = ctx
        .guild_id()
        .ok_or("This command requires a server.")?
        .get();
    let config = super::database::get_config(&ctx.data().counting_db, guild_id).await?;
    let rows = super::database::top_users(&ctx.data().counting_db, guild_id, 10).await?;
    if rows.is_empty() {
        ctx.say("No counting stats recorded yet!").await?;
        return Ok(());
    }
    let description = leaderboard_description(&rows, &config.currency_emoji, &config.currency_name);
    ctx.send(
        poise::CreateReply::default()
            .embed(
                CreateEmbed::new()
                    .title(format!(
                        "🏆 Counting Leaderboard ({})",
                        config.currency_name
                    ))
                    .description(description)
                    .color(0xFFD700),
            )
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await?;
    Ok(())
}

fn dashboard_components() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("counting:toggle_reset")
            .label("Toggle Fail Reset")
            .style(ButtonStyle::Primary),
        CreateButton::new("counting:currency")
            .label("Edit Currency")
            .style(ButtonStyle::Secondary),
        CreateButton::new("counting:rewards")
            .label("Edit Rewards / Penalties")
            .style(ButtonStyle::Secondary),
        CreateButton::new("counting:set_channel")
            .label("Set Current Channel")
            .style(ButtonStyle::Success),
    ])]
}

async fn dashboard_embed(data: &Data, guild_id: u64) -> Result<CreateEmbed, Error> {
    let config = super::database::get_config(&data.counting_db, guild_id).await?;
    let role = config
        .role_id
        .map(|id| format!("<@&{id}>"))
        .unwrap_or_else(|| "None".into());
    Ok(CreateEmbed::new()
        .title("⚙️ Counting System Dashboard")
        .color(0x3498DB)
        .field(
            "Channel",
            config
                .channel_id
                .map(|id| format!("<#{id}>"))
                .unwrap_or_else(|| "Not set".into()),
            true,
        )
        .field("Current Count", config.current_count.to_string(), true)
        .field(
            "Reset on Fail",
            if config.reset_on_fail {
                "Enabled"
            } else {
                "Disabled"
            },
            true,
        )
        .field(
            "Currency",
            format!("{} {}", config.currency_emoji, config.currency_name),
            true,
        )
        .field(
            "Penalty",
            format!("{} {}", config.penalty_amount, config.currency_name),
            true,
        )
        .field(
            "Rewards",
            format!(
                "Base: +{} | Math: +{}",
                config.base_reward, config.math_bonus
            ),
            true,
        )
        .field("Counter Role", role, false))
}

fn leaderboard_description(rows: &[super::database::UserStats], emoji: &str, name: &str) -> String {
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

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<(), Error> {
    let Some(guild_id) = interaction.guild_id.map(|id| id.get()) else {
        return respond(ctx, interaction, "This setting only works in a server.").await;
    };
    if !interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(|permissions| permissions.administrator())
    {
        return respond(
            ctx,
            interaction,
            "Only administrators can change counting settings.",
        )
        .await;
    }
    match interaction.data.custom_id.as_str() {
        "counting:toggle_reset" => {
            let config = super::database::get_config(&data.counting_db, guild_id).await?;
            sqlx::query("UPDATE guild_config SET reset_on_fail = ? WHERE guild_id = ?")
                .bind(i64::from(!config.reset_on_fail))
                .bind(guild_id as i64)
                .execute(&data.counting_db)
                .await?;
            let embed = dashboard_embed(data, guild_id).await?;
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(embed)
                            .components(dashboard_components()),
                    ),
                )
                .await?;
        }
        "counting:set_channel" => {
            sqlx::query("UPDATE guild_config SET channel_id = ? WHERE guild_id = ?")
                .bind(interaction.channel_id.get() as i64)
                .bind(guild_id as i64)
                .execute(&data.counting_db)
                .await?;
            let embed = dashboard_embed(data, guild_id).await?;
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .embed(embed)
                            .components(dashboard_components()),
                    ),
                )
                .await?;
        }
        "counting:currency" => {
            let config = super::database::get_config(&data.counting_db, guild_id).await?;
            let modal = CreateQuickModal::new("Configure Currency")
                .field(
                    CreateInputText::new(InputTextStyle::Short, "Currency Name", "e.g. Coins")
                        .value(config.currency_name),
                )
                .field(
                    CreateInputText::new(InputTextStyle::Short, "Currency Emoji / Icon", "e.g. 🪙")
                        .value(config.currency_emoji),
                );
            if let Some(response) = interaction.quick_modal(ctx, modal).await? {
                if response.inputs.len() >= 2 {
                    sqlx::query("UPDATE guild_config SET currency_name = ?, currency_emoji = ? WHERE guild_id = ?")
                        .bind(response.inputs[0].trim())
                        .bind(response.inputs[1].trim())
                        .bind(guild_id as i64)
                        .execute(&data.counting_db).await?;
                    respond_modal(ctx, &response.interaction, "Currency settings updated.").await?;
                }
            }
        }
        "counting:rewards" => {
            let config = super::database::get_config(&data.counting_db, guild_id).await?;
            let modal = CreateQuickModal::new("Configure Penalties & Rewards")
                .field(
                    CreateInputText::new(
                        InputTextStyle::Short,
                        "Penalty Amount",
                        "Non-negative integer",
                    )
                    .value(config.penalty_amount.to_string()),
                )
                .field(
                    CreateInputText::new(
                        InputTextStyle::Short,
                        "Base Count Reward",
                        "Non-negative integer",
                    )
                    .value(config.base_reward.to_string()),
                )
                .field(
                    CreateInputText::new(
                        InputTextStyle::Short,
                        "Math Bonus Reward",
                        "Non-negative integer",
                    )
                    .value(config.math_bonus.to_string()),
                );
            if let Some(response) = interaction.quick_modal(ctx, modal).await? {
                let values = response
                    .inputs
                    .iter()
                    .map(|input| input.trim().parse::<i64>())
                    .collect::<Result<Vec<_>, _>>();
                match values {
                    Ok(values) if values.len() == 3 && values.iter().all(|value| *value >= 0) => {
                        sqlx::query("UPDATE guild_config SET penalty_amount = ?, base_reward = ?, math_bonus = ? WHERE guild_id = ?")
                            .bind(values[0]).bind(values[1]).bind(values[2]).bind(guild_id as i64)
                            .execute(&data.counting_db).await?;
                        respond_modal(ctx, &response.interaction, "Rewards and penalties updated.")
                            .await?;
                    }
                    _ => {
                        respond_modal(
                            ctx,
                            &response.interaction,
                            "Enter non-negative whole numbers for all three settings.",
                        )
                        .await?
                    }
                }
            }
        }
        _ => return respond(ctx, interaction, "Unknown counting setting.").await,
    }
    Ok(())
}

async fn respond(
    ctx: &serenity::Context,
    interaction: &serenity::all::ComponentInteraction,
    text: &str,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(text)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}

async fn respond_modal(
    ctx: &serenity::Context,
    interaction: &serenity::all::ModalInteraction,
    text: &str,
) -> Result<(), Error> {
    interaction
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content(text)
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}
