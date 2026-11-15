use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use rand::Rng;

/// Roll one or more dice.
#[poise::command(slash_command)]
pub async fn roll(
    ctx: Context<'_>,
    #[description = "Number of sides on each die (2–1000)"] sides: Option<u16>,
    #[description = "How many dice to roll (1–25)"] dice: Option<u8>,
    #[description = "Post the result publicly"] public: Option<bool>,
) -> Result<(), Error> {
    let sides = sides.unwrap_or(6);
    let dice = dice.unwrap_or(1);
    if !(2..=1000).contains(&sides) || !(1..=25).contains(&dice) {
        ctx.say("Sides must be between 2 and 1000, and dice must be between 1 and 25.")
            .await?;
        return Ok(());
    }

    let rolls = {
        let mut rng = rand::thread_rng();
        (0..dice)
            .map(|_| rng.gen_range(1..=sides))
            .collect::<Vec<_>>()
    };
    let total: u32 = rolls.iter().map(|roll| u32::from(*roll)).sum();
    let maximum = u32::from(sides) * u32::from(dice);
    let minimum = u32::from(dice);
    let ratio = (total - minimum) as f64 / (maximum - minimum).max(1) as f64;
    let (quality, color) = if ratio == 1.0 {
        ("🏆 Perfect roll!", serenity::Colour::from_rgb(255, 215, 0))
    } else if ratio >= 0.75 {
        ("✨ Great roll!", serenity::Colour::from_rgb(87, 242, 135))
    } else if ratio >= 0.4 {
        ("🎲 Decent roll", serenity::Colour::from_rgb(254, 231, 92))
    } else {
        ("💀 Rough roll…", serenity::Colour::from_rgb(237, 66, 69))
    };
    let result = rolls
        .iter()
        .map(|roll| format!("〖**{roll}**〗"))
        .collect::<Vec<_>>()
        .join("  ");
    let embed = serenity::CreateEmbed::new()
        .title(format!("🎲 Rolling {dice}d{sides} — {quality}"))
        .description(format!(
            "**Results**\n{result}\n\n**Total:** {total} • **Range:** {minimum}–{maximum}"
        ))
        .color(color)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Rolled by {}",
            ctx.author().name
        )));
    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .ephemeral(!public.unwrap_or(false)),
    )
    .await?;
    Ok(())
}

/// Flip a coin.
#[poise::command(slash_command)]
pub async fn coinflip(
    ctx: Context<'_>,
    #[description = "Post the result publicly"] public: Option<bool>,
) -> Result<(), Error> {
    let heads = rand::thread_rng().gen_bool(0.5);
    let side = if heads { "Heads!" } else { "Tails!" };
    let color = if heads {
        serenity::Colour::from_rgb(255, 215, 0)
    } else {
        serenity::Colour::from_rgb(178, 190, 195)
    };
    let embed = serenity::CreateEmbed::new()
        .title(format!("🪙 {side}"))
        .color(color)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Flipped by {}",
            ctx.author().name
        )));
    ctx.send(
        poise::CreateReply::default()
            .embed(embed)
            .ephemeral(!public.unwrap_or(false)),
    )
    .await?;
    Ok(())
}
