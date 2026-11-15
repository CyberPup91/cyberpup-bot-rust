use crate::{Context, Error};
use poise::serenity_prelude as serenity;

#[derive(poise::Modal)]
#[name = "Create Reaction Role Panel"]
struct CreatePanelModal {
    #[name = "Panel title"]
    #[placeholder = "Choose your roles"]
    title: String,
    #[name = "Choices: emoji - @role, one per line"]
    #[placeholder = "🎨 - <@&123456789012345678>\n🌟 - <@&234567890123456789>"]
    #[paragraph]
    body: String,
    #[name = "Maximum selected roles (0 = unlimited)"]
    #[placeholder = "0"]
    max_selections: Option<String>,
    #[name = "Embed color (optional hex)"]
    #[placeholder = "5865F2"]
    color: Option<String>,
}

/// Create reaction-role panels.
#[poise::command(slash_command, guild_only, subcommands("create"))]
pub async fn reactionpanel(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Open a form to create and post a reaction-role panel in this channel.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_ROLES")]
pub async fn create(ctx: Context<'_>) -> Result<(), Error> {
    let poise::Context::Application(ctx) = ctx else {
        return Ok(());
    };
    let Some(form) = poise::execute_modal::<_, _, CreatePanelModal>(ctx, None, None).await? else {
        return Ok(());
    };
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("This command requires a server.")?;
    let maximum = match form
        .max_selections
        .as_deref()
        .unwrap_or("0")
        .trim()
        .parse::<u8>()
    {
        Ok(maximum) => maximum,
        Err(_) => {
            ctx.say("Maximum selections must be a number from 0 to 25.")
                .await?;
            return Ok(());
        }
    };
    if maximum > 25 {
        ctx.say("Maximum selections must be between 0 and 25; use 0 for unlimited.")
            .await?;
        return Ok(());
    }
    let title = form.title.trim();
    if title.is_empty() {
        ctx.say("The panel title cannot be empty.").await?;
        return Ok(());
    }
    let body = form.body.trim();
    let bindings = super::parse_bindings(body);
    if bindings.is_empty() {
        ctx.say("No valid reaction and role lines found. Use a format like `🎨 - <@&123456789012345678>`.")
            .await?;
        return Ok(());
    }
    if bindings.len() > 20 {
        ctx.say("A reaction-role panel can have at most 20 role choices.")
            .await?;
        return Ok(());
    }

    let channel_id = ctx.channel_id();
    let embed = serenity::CreateEmbed::new()
        .title(title)
        .description(body)
        .color(super::parse_color(form.color.as_deref()));
    let message = channel_id
        .send_message(
            ctx,
            serenity::CreateMessage::new()
                .embed(embed)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await?;

    let mut active_bindings = Vec::new();
    let mut failed = Vec::new();
    for (emoji, role_id) in bindings {
        let reaction = match serenity::all::ReactionType::try_from(emoji.as_str()) {
            Ok(reaction) => reaction,
            Err(_) => continue,
        };
        match message.react(ctx, reaction).await {
            Ok(_) => active_bindings.push((emoji, role_id)),
            Err(error) => failed.push(format!("{emoji}: {error}")),
        }
    }
    if active_bindings.is_empty() {
        ctx.say("The panel was posted, but I could not add any reactions. Check my Add Reactions permission and emoji access.")
            .await?;
        return Ok(());
    }
    super::database::create_panel(
        &ctx.data().reaction_roles_db,
        message.id.get(),
        guild_id.get(),
        channel_id.get(),
        maximum,
        &active_bindings,
    )
    .await?;
    let mut response = format!(
        "Reaction-role panel posted with {} choices: {}",
        active_bindings.len(),
        message.link()
    );
    if !failed.is_empty() {
        response.push_str(&format!("\nCould not add {} reaction(s).", failed.len()));
    }
    ctx.say(response).await?;
    Ok(())
}
