use poise::serenity_prelude as serenity;
use serenity::all::{
    ChannelId, Colour, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage, Guild,
    Timestamp, User,
};
use sqlx::SqlitePool;

use super::database::GreeterConfig;

/// Build a preview embed using the configured message and placeholders.
pub fn build_test_embed(
    config: &GreeterConfig,
    user: &User,
    guild: &Guild,
    is_join: bool,
) -> CreateEmbed {
    let (title, description, footer, color) = if is_join {
        (
            &config.join_title,
            &config.join_description,
            &config.join_footer,
            &config.join_color,
        )
    } else {
        (
            &config.leave_title,
            &config.leave_description,
            &config.leave_footer,
            &config.leave_color,
        )
    };

    let username = &user.name;
    let display_name = user.global_name.as_deref().unwrap_or(username);

    let discriminator = user
        .discriminator
        .map(|value| value.get().to_string())
        .unwrap_or_else(|| "0".to_string());

    let member_count = format_number(guild.member_count);

    let replacements = [
        ("{user}", format!("<@{}>", user.id.get())),
        ("{username}", username.clone()),
        ("{discriminator}", discriminator),
        ("{display_name}", display_name.to_string()),
        ("{id}", user.id.get().to_string()),
        ("{guild}", guild.name.clone()),
        ("{guild_owner}", format!("<@{}>", guild.owner_id.get())),
        ("{member_count}", member_count),
        ("{joined_at}", "Unknown".to_string()),
    ];

    let mut formatted_title = title.clone();
    let mut formatted_description = description.clone();
    let mut formatted_footer = footer.clone();

    for (placeholder, value) in &replacements {
        formatted_title = formatted_title.replace(placeholder, value);
        formatted_description = formatted_description.replace(placeholder, value);
        formatted_footer = formatted_footer.replace(placeholder, value);
    }

    CreateEmbed::new()
        .title(formatted_title)
        .description(formatted_description)
        .colour(parse_color(color))
        .timestamp(Timestamp::now())
        .footer(CreateEmbedFooter::new(formatted_footer))
}

/// Resolve named colors or six-digit hexadecimal colors.
fn parse_color(value: &str) -> Colour {
    let normalized = value.trim().to_lowercase();

    let named = match normalized.as_str() {
        "red" => Some((255, 0, 0)),
        "green" => Some((0, 255, 0)),
        "blue" => Some((0, 0, 255)),
        "purple" => Some((128, 0, 128)),
        "orange" => Some((255, 165, 0)),
        "yellow" => Some((255, 255, 0)),
        "dark_red" => Some((139, 0, 0)),
        "dark_green" => Some((0, 100, 0)),
        "dark_blue" => Some((0, 0, 139)),
        "dark_purple" => Some((75, 0, 130)),
        "dark_gray" | "dark_grey" => Some((64, 64, 64)),
        "blurple" => Some((88, 101, 242)),
        "default" => Some((47, 49, 54)),
        _ => None,
    };

    if let Some((r, g, b)) = named {
        return Colour::from_rgb(r, g, b);
    }

    let hex = normalized
        .strip_prefix('#')
        .or_else(|| normalized.strip_prefix("0x"))
        .unwrap_or(&normalized);

    if hex.len() == 6 {
        if let Ok(rgb) = u32::from_str_radix(hex, 16) {
            return Colour::from_rgb(
                ((rgb >> 16) & 0xFF) as u8,
                ((rgb >> 8) & 0xFF) as u8,
                (rgb & 0xFF) as u8,
            );
        }
    }

    // Match the original Python default.
    Colour::from_rgb(128, 0, 128)
}

/// Format an integer with thousands separators.
fn format_number(number: u64) -> String {
    let digits = number.to_string();
    let mut formatted = String::new();

    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(character);
    }

    formatted
}

/// Send a configured join or leave message for a guild.
pub async fn send_greeter(
    ctx: &serenity::Context,
    pool: &SqlitePool,
    guild: &Guild,
    user: &User,
    is_join: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config = super::database::get_config(pool, guild.id.get()).await?;

    let (channel_id, enabled) = if is_join {
        (config.join_channel_id, config.join_enabled)
    } else {
        (config.leave_channel_id, config.leave_enabled)
    };

    // Respect the per-guild enabled setting.
    if !enabled {
        return Ok(());
    }

    // Nothing to send if no channel has been configured.
    let Some(channel_id) = channel_id else {
        return Ok(());
    };

    let embed = build_test_embed(&config, user, guild, is_join);

    ChannelId::new(channel_id)
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .embed(embed)
                .allowed_mentions(CreateAllowedMentions::new()),
        )
        .await?;

    Ok(())
}
