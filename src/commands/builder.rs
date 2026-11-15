use crate::{Context, Data, Error};
use poise::serenity_prelude as serenity;
use serde::{Deserialize, Serialize};
use serenity::all::{
    ButtonStyle, CreateActionRow, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu,
    CreateSelectMenuKind, CreateSelectMenuOption, EditInteractionResponse, EditMessage,
    InputTextStyle,
};
use serenity::utils::CreateQuickModal;
use serenity::Mentionable;
use std::time::Duration;

pub mod database;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuilderDraft {
    pub channel_id: u64,
    pub target_message_id: Option<u64>,
    pub title: String,
    pub description: String,
    pub content: String,
    pub color: u32,
    pub thumbnail_url: String,
    pub image_url: String,
    pub author_name: String,
    pub footer_text: String,
    pub fields: Vec<BuilderField>,
    pub allow_pings: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuilderField {
    pub name: String,
    pub value: String,
}

pub async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    database::init_db(pool).await
}

/// Open an interactive card builder, preview the draft, and publish it when ready.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_MESSAGES")]
pub async fn build(
    ctx: Context<'_>,
    #[description = "Channel to publish to; defaults to this channel"] channel: Option<
        serenity::ChannelId,
    >,
) -> Result<(), Error> {
    let draft = BuilderDraft {
        channel_id: channel.unwrap_or_else(|| ctx.channel_id()).get(),
        target_message_id: None,
        title: "New Layout".into(),
        description: "Use Edit Card to customize this draft.".into(),
        content: String::new(),
        color: 0x5865F2,
        thumbnail_url: String::new(),
        image_url: String::new(),
        author_name: String::new(),
        footer_text: String::new(),
        fields: Vec::new(),
        allow_pings: false,
    };
    start_builder(ctx, draft).await
}

/// Open the builder with an existing bot-authored message.
#[poise::command(slash_command, guild_only, required_permissions = "MANAGE_MESSAGES")]
pub async fn edit_message(
    ctx: Context<'_>,
    #[description = "Discord message link"] message_link: String,
) -> Result<(), Error> {
    let parts = message_link
        .trim_end_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let Some(message_id) = parts.last().and_then(|value| value.parse::<u64>().ok()) else {
        ctx.say("Could not read a message ID from that link.")
            .await?;
        return Ok(());
    };
    let Some(channel_id) = parts
        .iter()
        .rev()
        .nth(1)
        .and_then(|value| value.parse::<u64>().ok())
    else {
        ctx.say("Could not read a channel ID from that link.")
            .await?;
        return Ok(());
    };
    let channel_id = serenity::ChannelId::new(channel_id);
    let message = match channel_id
        .message(ctx.serenity_context(), serenity::MessageId::new(message_id))
        .await
    {
        Ok(message) => message,
        Err(error) => {
            ctx.say(format!("Could not fetch that message: {error}"))
                .await?;
            return Ok(());
        }
    };
    let bot_id = ctx.serenity_context().cache.current_user().id;
    if message.author.id != bot_id {
        ctx.say("I can only edit messages authored by this bot.")
            .await?;
        return Ok(());
    }
    start_builder(ctx, draft_from_message(&message, Some(message_id))).await
}

/// Edit a bot message directly from Discord's message context menu.
#[poise::command(
    context_menu_command = "Edit Layout",
    slash_command,
    guild_only,
    required_permissions = "MANAGE_MESSAGES"
)]
pub async fn edit_layout(ctx: Context<'_>, message: serenity::Message) -> Result<(), Error> {
    let bot_id = ctx.serenity_context().cache.current_user().id;
    if message.author.id != bot_id {
        ctx.send(
            poise::CreateReply::default()
                .content("I can only edit messages authored by this bot.")
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    }
    let draft = draft_from_message(&message, Some(message.id.get()));
    start_builder(ctx, draft).await
}

fn draft_from_message(message: &serenity::Message, target_message_id: Option<u64>) -> BuilderDraft {
    let embed = message.embeds.first();
    BuilderDraft {
        channel_id: message.channel_id.get(),
        target_message_id,
        title: embed
            .and_then(|embed| embed.title.clone())
            .unwrap_or_else(|| "Edit Layout".into()),
        description: embed
            .and_then(|embed| embed.description.clone())
            .unwrap_or_default(),
        content: message.content.clone(),
        color: embed
            .and_then(|embed| embed.colour)
            .map(|colour| colour.0)
            .unwrap_or(0x5865F2),
        thumbnail_url: embed
            .and_then(|embed| embed.thumbnail.as_ref())
            .map(|image| image.url.clone())
            .unwrap_or_default(),
        image_url: embed
            .and_then(|embed| embed.image.as_ref())
            .map(|image| image.url.clone())
            .unwrap_or_default(),
        author_name: embed
            .and_then(|embed| embed.author.as_ref())
            .map(|author| author.name.clone())
            .unwrap_or_default(),
        footer_text: embed
            .and_then(|embed| embed.footer.as_ref())
            .map(|footer| footer.text.clone())
            .unwrap_or_default(),
        fields: embed
            .map(|embed| {
                embed
                    .fields
                    .iter()
                    .map(|field| BuilderField {
                        name: field.name.clone(),
                        value: field.value.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        allow_pings: false,
    }
}

async fn start_builder(ctx: Context<'_>, draft: BuilderDraft) -> Result<(), Error> {
    let owner_id = ctx.author().id.get();
    ctx.data()
        .builder_sessions
        .lock()
        .await
        .insert(owner_id, draft.clone());
    ctx.send(
        poise::CreateReply::default()
            .content(&draft.content)
            .allowed_mentions(allowed_mentions(false))
            .embed(preview_embed(&draft))
            .components(builder_components(&draft))
            .ephemeral(true),
    )
    .await?;
    Ok(())
}

pub async fn handle_component(
    ctx: &serenity::Context,
    data: &Data,
    interaction: &serenity::all::ComponentInteraction,
) -> Result<bool, Error> {
    if !interaction.data.custom_id.starts_with("builder:") {
        return Ok(false);
    }
    let owner_id = interaction.user.id.get();
    let draft = data.builder_sessions.lock().await.get(&owner_id).cloned();
    let Some(mut draft) = draft else {
        interaction
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This builder draft expired. Run `/build` to start another.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(true);
    };

    match interaction.data.custom_id.as_str() {
        "builder:edit" => {
            let modal = CreateQuickModal::new("Edit Card")
                .timeout(Duration::from_secs(300))
                .field(
                    CreateInputText::new(InputTextStyle::Short, "Title", "Card title")
                        .value(draft.title.clone())
                        .max_length(256),
                )
                .field(
                    CreateInputText::new(
                        InputTextStyle::Paragraph,
                        "Description",
                        "Main card text",
                    )
                    .value(draft.description.clone())
                    .max_length(4000),
                )
                .field(
                    CreateInputText::new(
                        InputTextStyle::Paragraph,
                        "Message content (optional)",
                        "Text above the embed",
                    )
                    .value(draft.content.clone())
                    .required(false)
                    .max_length(2000),
                )
                .field(
                    CreateInputText::new(InputTextStyle::Short, "Color (hex)", "5865F2")
                        .value(format!("{:06X}", draft.color))
                        .required(false)
                        .max_length(7),
                )
                .field(
                    CreateInputText::new(
                        InputTextStyle::Paragraph,
                        "Image URLs (optional)",
                        "Thumbnail URL on line 1, large image URL on line 2",
                    )
                    .value(format!("{}\n{}", draft.thumbnail_url, draft.image_url))
                    .required(false)
                    .max_length(1000),
                );
            if let Some(response) = interaction.quick_modal(ctx, modal).await? {
                if response.inputs.len() < 5 {
                    return Ok(true);
                }
                draft.title = response.inputs[0].trim().to_owned();
                draft.description = response.inputs[1].trim().to_owned();
                draft.content = response.inputs[2].trim().to_owned();
                if !response.inputs[3].trim().is_empty() {
                    if let Ok(color) =
                        u32::from_str_radix(response.inputs[3].trim().trim_start_matches('#'), 16)
                    {
                        draft.color = color.min(0xFFFFFF);
                    }
                }
                let mut images = response.inputs[4].lines();
                draft.thumbnail_url = images.next().unwrap_or_default().trim().to_owned();
                draft.image_url = images.next().unwrap_or_default().trim().to_owned();
                let mut sessions = data.builder_sessions.lock().await;
                sessions.insert(owner_id, draft.clone());
                response
                    .interaction
                    .create_response(
                        ctx,
                        CreateInteractionResponse::UpdateMessage(
                            CreateInteractionResponseMessage::new()
                                .content(&draft.content)
                                .allowed_mentions(allowed_mentions(draft.allow_pings))
                                .embed(preview_embed(&draft))
                                .components(builder_components(&draft)),
                        ),
                    )
                    .await?;
            }
        }
        "builder:details" => {
            let fields = draft
                .fields
                .iter()
                .map(|field| format!("{} | {}", field.name, field.value))
                .collect::<Vec<_>>()
                .join("\n");
            let modal = CreateQuickModal::new("Edit Card Details")
                .timeout(Duration::from_secs(300))
                .field(
                    CreateInputText::new(InputTextStyle::Short, "Author / subtitle", "Optional")
                        .value(draft.author_name.clone())
                        .required(false)
                        .max_length(256),
                )
                .field(
                    CreateInputText::new(InputTextStyle::Short, "Footer", "Optional")
                        .value(draft.footer_text.clone())
                        .required(false)
                        .max_length(2048),
                )
                .field(
                    CreateInputText::new(
                        InputTextStyle::Paragraph,
                        "Section fields: name | value",
                        "One field per line; leave empty to clear",
                    )
                    .value(fields)
                    .required(false)
                    .max_length(4000),
                );
            if let Some(response) = interaction.quick_modal(ctx, modal).await? {
                if response.inputs.len() >= 3 {
                    draft.author_name = response.inputs[0].trim().to_owned();
                    draft.footer_text = response.inputs[1].trim().to_owned();
                    let mut fields = Vec::new();
                    for line in response.inputs[2]
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                    {
                        if fields.len() >= 25 {
                            break;
                        }
                        if let Some((name, value)) = line.split_once('|') {
                            let field = BuilderField {
                                name: name.trim().to_owned(),
                                value: value.trim().to_owned(),
                            };
                            if !field.name.is_empty() && !field.value.is_empty() {
                                fields.push(field);
                            }
                        }
                    }
                    draft.fields = fields;
                    data.builder_sessions
                        .lock()
                        .await
                        .insert(owner_id, draft.clone());
                    response
                        .interaction
                        .create_response(
                            ctx,
                            CreateInteractionResponse::UpdateMessage(
                                CreateInteractionResponseMessage::new()
                                    .content(&draft.content)
                                    .allowed_mentions(allowed_mentions(draft.allow_pings))
                                    .embed(preview_embed(&draft))
                                    .components(builder_components(&draft)),
                            ),
                        )
                        .await?;
                }
            }
        }
        "builder:save" => {
            let modal = CreateQuickModal::new("Save Builder Template")
                .timeout(Duration::from_secs(300))
                .short_field("Template name");
            if let Some(response) = interaction.quick_modal(ctx, modal).await? {
                let name = response
                    .inputs
                    .first()
                    .map(|value| value.trim())
                    .unwrap_or_default();
                if name.is_empty() {
                    response
                        .interaction
                        .create_response(
                            ctx,
                            CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new()
                                    .content("Template name cannot be empty.")
                                    .ephemeral(true),
                            ),
                        )
                        .await?;
                } else {
                    database::save_template(&data.db, name, &draft).await?;
                    response
                        .interaction
                        .create_response(
                            ctx,
                            CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new()
                                    .content(format!("Saved template `{name}`."))
                                    .ephemeral(true),
                            ),
                        )
                        .await?;
                }
            }
        }
        "builder:load" => {
            let templates = database::list_templates(&data.db).await?;
            if templates.is_empty() {
                interaction
                    .create_response(
                        ctx,
                        CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .content("No saved templates yet.")
                                .ephemeral(true),
                        ),
                    )
                    .await?;
            } else {
                let options = templates
                    .into_iter()
                    .map(|name| {
                        CreateSelectMenuOption::new(
                            name.chars().take(100).collect::<String>(),
                            name,
                        )
                    })
                    .collect();
                let menu = CreateSelectMenu::new(
                    "builder:load-select",
                    CreateSelectMenuKind::String { options },
                )
                .placeholder("Choose a saved template");
                interaction
                    .create_response(
                        ctx,
                        CreateInteractionResponse::UpdateMessage(
                            CreateInteractionResponseMessage::new()
                                .content("Choose a template to load:")
                                .components(vec![CreateActionRow::SelectMenu(menu)]),
                        ),
                    )
                    .await?;
            }
        }
        "builder:load-select" => {
            let selected = match &interaction.data.kind {
                serenity::all::ComponentInteractionDataKind::StringSelect { values } => {
                    values.first()
                }
                _ => None,
            };
            if let Some(name) = selected {
                if let Some(mut template) = database::get_template(&data.db, name).await? {
                    template.channel_id = draft.channel_id;
                    template.target_message_id = draft.target_message_id;
                    template.allow_pings = draft.allow_pings;
                    data.builder_sessions
                        .lock()
                        .await
                        .insert(owner_id, template.clone());
                    interaction
                        .create_response(
                            ctx,
                            CreateInteractionResponse::UpdateMessage(
                                CreateInteractionResponseMessage::new()
                                    .content(&template.content)
                                    .allowed_mentions(allowed_mentions(template.allow_pings))
                                    .embed(preview_embed(&template))
                                    .components(builder_components(&template)),
                            ),
                        )
                        .await?;
                } else {
                    interaction
                        .create_response(
                            ctx,
                            CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new()
                                    .content("That template no longer exists.")
                                    .ephemeral(true),
                            ),
                        )
                        .await?;
                }
            }
        }
        "builder:pings" => {
            draft.allow_pings = !draft.allow_pings;
            data.builder_sessions
                .lock()
                .await
                .insert(owner_id, draft.clone());
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .content(&draft.content)
                            .allowed_mentions(allowed_mentions(draft.allow_pings))
                            .embed(preview_embed(&draft))
                            .components(builder_components(&draft)),
                    ),
                )
                .await?;
        }
        "builder:publish" => {
            let channel_id = serenity::ChannelId::new(draft.channel_id);
            let embed = preview_embed(&draft);
            let mentions = allowed_mentions(draft.allow_pings);
            let posted = if let Some(message_id) = draft.target_message_id {
                channel_id
                    .edit_message(
                        ctx,
                        serenity::MessageId::new(message_id),
                        EditMessage::new()
                            .content(&draft.content)
                            .embeds(vec![embed])
                            .allowed_mentions(mentions),
                    )
                    .await?
            } else {
                let mut message = CreateMessage::new().embed(embed).allowed_mentions(mentions);
                if !draft.content.is_empty() {
                    message = message.content(&draft.content);
                }
                channel_id.send_message(ctx, message).await?
            };

            if let Some(guild_id) = interaction.guild_id {
                let mut binding_text = draft.description.clone();
                binding_text.push_str(
                    &draft
                        .fields
                        .iter()
                        .map(|field| field.value.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                let bindings = super::reaction_roles::parse_bindings(&binding_text);
                let mut active = Vec::new();
                for (emoji, role_id) in bindings {
                    if let Ok(reaction) = serenity::all::ReactionType::try_from(emoji.as_str()) {
                        if posted.react(ctx, reaction).await.is_ok() {
                            active.push((emoji, role_id));
                        }
                    }
                }
                if !active.is_empty() {
                    super::reaction_roles::database::create_panel(
                        &data.reaction_roles_db,
                        posted.id.get(),
                        guild_id.get(),
                        channel_id.get(),
                        0,
                        &active,
                    )
                    .await?;
                } else if draft.target_message_id.is_some() {
                    super::reaction_roles::database::remove_panel(
                        &data.reaction_roles_db,
                        posted.id.get(),
                    )
                    .await?;
                }
            }
            data.builder_sessions.lock().await.remove(&owner_id);
            let result = if draft.target_message_id.is_some() {
                "Updated the existing bot message."
            } else {
                "Published your card."
            };
            interaction
                .edit_response(
                    ctx,
                    EditInteractionResponse::new()
                        .content(format!("{result} Destination: {}", channel_id.mention()))
                        .components(Vec::new()),
                )
                .await?;
        }
        "builder:cancel" => {
            data.builder_sessions.lock().await.remove(&owner_id);
            interaction
                .edit_response(
                    ctx,
                    EditInteractionResponse::new()
                        .content("Builder draft cancelled.")
                        .components(Vec::new()),
                )
                .await?;
        }
        _ => {
            interaction
                .create_response(
                    ctx,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("Unknown builder action.")
                            .ephemeral(true),
                    ),
                )
                .await?;
        }
    }
    Ok(true)
}

fn preview_embed(draft: &BuilderDraft) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title(&draft.title)
        .description(if draft.description.is_empty() {
            "*(No description)*"
        } else {
            &draft.description
        })
        .color(serenity::Colour::new(draft.color));
    if valid_image_url(&draft.thumbnail_url) {
        embed = embed.thumbnail(&draft.thumbnail_url);
    }
    if valid_image_url(&draft.image_url) {
        embed = embed.image(&draft.image_url);
    }
    if !draft.author_name.is_empty() {
        embed = embed.author(serenity::CreateEmbedAuthor::new(&draft.author_name));
    }
    for field in draft.fields.iter().take(25) {
        embed = embed.field(&field.name, &field.value, false);
    }
    if !draft.footer_text.is_empty() {
        embed = embed.footer(serenity::CreateEmbedFooter::new(&draft.footer_text));
    }
    embed
}

fn valid_image_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn allowed_mentions(allow_pings: bool) -> serenity::CreateAllowedMentions {
    serenity::CreateAllowedMentions::new()
        .all_users(allow_pings)
        .all_roles(allow_pings)
        .everyone(allow_pings)
}

fn builder_components(draft: &BuilderDraft) -> Vec<CreateActionRow> {
    vec![
        CreateActionRow::Buttons(vec![
            CreateButton::new("builder:edit")
                .label("Edit Card")
                .style(ButtonStyle::Primary),
            CreateButton::new("builder:details")
                .label("Details & Fields")
                .style(ButtonStyle::Secondary),
            CreateButton::new("builder:pings")
                .label(if draft.allow_pings {
                    "Pings: ON"
                } else {
                    "Pings: OFF"
                })
                .style(if draft.allow_pings {
                    ButtonStyle::Success
                } else {
                    ButtonStyle::Secondary
                }),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new("builder:save")
                .label("Save Template")
                .style(ButtonStyle::Secondary),
            CreateButton::new("builder:load")
                .label("Load Template")
                .style(ButtonStyle::Secondary),
            CreateButton::new("builder:publish")
                .label(if draft.target_message_id.is_some() {
                    "Update Message"
                } else {
                    "Publish"
                })
                .style(ButtonStyle::Success),
            CreateButton::new("builder:cancel")
                .label("Cancel")
                .style(ButtonStyle::Danger),
        ]),
    ]
}
