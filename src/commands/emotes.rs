use crate::{Context, Error};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use poise::serenity_prelude as serenity;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

const MAX_IMAGE_BYTES: usize = 255_000;

#[derive(Debug)]
struct DiscordEmote {
    name: String,
    id: String,
    animated: bool,
}

struct DownloadedEmote {
    name: String,
    bytes: Vec<u8>,
    mime: &'static str,
}

struct StorageTarget {
    id: serenity::GuildId,
    name: String,
    static_remaining_slots: usize,
    animated_remaining_slots: usize,
}

impl StorageTarget {
    fn remaining_slots(&self, animated: bool) -> usize {
        if animated {
            self.animated_remaining_slots
        } else {
            self.static_remaining_slots
        }
    }

    fn consume_slot(&mut self, animated: bool) {
        if animated {
            self.animated_remaining_slots = self.animated_remaining_slots.saturating_sub(1);
        } else {
            self.static_remaining_slots = self.static_remaining_slots.saturating_sub(1);
        }
    }
}

fn env_ids(key: &str) -> HashSet<u64> {
    std::env::var(key)
        .unwrap_or_default()
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect()
}

#[allow(deprecated)]
async fn is_trusted(ctx: Context<'_>) -> bool {
    let user_id = ctx.author().id.get();

    if env_ids("TRUSTED_USER_IDS").contains(&user_id) {
        return true;
    }

    let Some(member) = ctx.author_member().await else {
        return false;
    };

    if member
        .roles
        .iter()
        .any(|role| env_ids("ALLOWED_ROLE_IDS").contains(&role.get()))
    {
        return true;
    }

    member
        .permissions(&ctx.serenity_context().cache)
        .is_ok_and(|permissions| permissions.administrator())
}

fn is_trusted_user(ctx: Context<'_>) -> bool {
    env_ids("TRUSTED_USER_IDS").contains(&ctx.author().id.get())
}

fn unauthorized() -> poise::CreateReply {
    poise::CreateReply::default()
        .content("🛑 Unauthorized.")
        .ephemeral(true)
}

fn discord_emote_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();

    REGEX.get_or_init(|| {
        Regex::new(r"<(a?):([A-Za-z0-9_]{2,32}):(\d+)>").expect("Discord emoji regex is valid")
    })
}

/// Returns Some(emojis) for Discord emoji input, None for other sources.
fn parse_discord_emotes(input: &str) -> Result<Option<Vec<DiscordEmote>>, String> {
    if !input.contains('<') {
        return Ok(None);
    }

    let regex = discord_emote_regex();
    let mut emojis = Vec::new();
    let mut previous_end = 0;

    for captures in regex.captures_iter(input) {
        let whole = captures.get(0).expect("full regex match");
        let between = &input[previous_end..whole.start()];

        if !between.chars().all(|c| c.is_whitespace() || c == ',') {
            return Err(
                "Please provide only Discord emoji markup, separated by spaces or commas."
                    .to_string(),
            );
        }

        emojis.push(DiscordEmote {
            animated: &captures[1] == "a",
            name: captures[2].to_string(),
            id: captures[3].to_string(),
        });

        previous_end = whole.end();
    }

    if emojis.is_empty() {
        return Err("Invalid Discord emoji. Use `<:name:id>` or `<a:name:id>`.".to_string());
    }

    let remaining = &input[previous_end..];

    if !remaining.chars().all(|c| c.is_whitespace() || c == ',') {
        return Err(
            "Please provide only Discord emoji markup, separated by spaces or commas.".to_string(),
        );
    }

    Ok(Some(emojis))
}

fn sanitize_emoji_name(input: &str) -> String {
    let mut name: String = input
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .take(32)
        .collect();

    while name.starts_with('_') {
        name.remove(0);
    }

    while name.ends_with('_') {
        name.pop();
    }

    if name.len() < 2 {
        name = format!("em_{name}");
    }

    name
}

/// Autocomplete only exposes configured storage servers to trusted users.
async fn storage_guild_autocomplete(
    ctx: Context<'_>,
    partial: &str,
) -> serenity::CreateAutocompleteResponse {
    if !is_trusted(ctx).await {
        return serenity::CreateAutocompleteResponse::new();
    }

    let http = &ctx.serenity_context().http;
    let partial_lower = partial.to_lowercase();

    let mut choices = Vec::new();

    for raw_id in env_ids("EMOTE_STORAGE_GUILD_IDS") {
        if choices.len() >= 25 {
            break;
        }

        let guild_id = serenity::GuildId::new(raw_id);

        let Ok(guild) = guild_id.to_partial_guild(http).await else {
            continue;
        };

        let label = format!("{} ({})", guild.name, raw_id);

        if !partial_lower.is_empty()
            && !guild.name.to_lowercase().contains(&partial_lower)
            && !raw_id.to_string().contains(&partial_lower)
        {
            continue;
        }

        choices.push(serenity::AutocompleteChoice::new(label, raw_id.to_string()));
    }

    serenity::CreateAutocompleteResponse::new().set_choices(choices)
}

/// Validates a guild destination and checks the bot's permissions/capacity.
async fn validate_guild_target(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
) -> Result<StorageTarget, String> {
    let http = &ctx.serenity_context().http;

    let guild = guild_id.to_partial_guild(http).await.map_err(|_| {
        "I couldn't access that server. Make sure the bot is installed there.".to_string()
    })?;

    let bot_user = http
        .get_current_user()
        .await
        .map_err(|_| "I couldn't identify my bot account.".to_string())?;

    let bot_member = guild.member(http, bot_user.id).await.map_err(|_| {
        format!(
            "I'm not a member of **{}**. Please install the bot in that server.",
            guild.name
        )
    })?;

    let permissions = guild.member_permissions(&bot_member);

    if !permissions.contains(serenity::Permissions::CREATE_GUILD_EXPRESSIONS)
        && !permissions.contains(serenity::Permissions::ADMINISTRATOR)
    {
        return Err(format!(
            "I don't have the **Create Expressions** permission in **{}**.",
            guild.name
        ));
    }

    // Discord provides separate limits for static and animated emoji.
    let emoji_limit: usize = match guild.premium_tier {
        serenity::PremiumTier::Tier0 => 50,
        serenity::PremiumTier::Tier1 => 100,
        serenity::PremiumTier::Tier2 => 150,
        serenity::PremiumTier::Tier3 => 250,
        _ => 50,
    };

    let static_count = guild
        .emojis
        .values()
        .filter(|emoji| !emoji.animated)
        .count();

    let animated_count = guild.emojis.values().filter(|emoji| emoji.animated).count();

    let static_remaining_slots = emoji_limit.saturating_sub(static_count);
    let animated_remaining_slots = emoji_limit.saturating_sub(animated_count);

    tracing::info!(
        "[Emotes] Capacity: guild={} name={} tier={:?} static={}/{} animated={}/{}",
        guild_id,
        guild.name,
        guild.premium_tier,
        static_count,
        emoji_limit,
        animated_count,
        emoji_limit
    );

    Ok(StorageTarget {
        id: guild_id,
        name: guild.name,
        static_remaining_slots,
        animated_remaining_slots,
    })
}

/// Validates a configured storage server.
async fn validate_storage_target(ctx: Context<'_>, raw_id: &str) -> Result<StorageTarget, String> {
    let guild_id_value: u64 = raw_id
        .parse()
        .map_err(|_| "Please select a valid storage server from the suggestions.".to_string())?;

    if !env_ids("EMOTE_STORAGE_GUILD_IDS").contains(&guild_id_value) {
        return Err("That server is not in the configured emoji storage allowlist.".to_string());
    }

    validate_guild_target(ctx, serenity::GuildId::new(guild_id_value)).await
}

/// Validates the server where the command was invoked.
async fn validate_current_target(ctx: Context<'_>) -> Result<StorageTarget, String> {
    let guild_id = ctx.guild_id().ok_or_else(|| {
        "This command must be run inside a server when no storage destination is selected."
            .to_string()
    })?;

    // Explicitly trusted accounts can use the current server without
    // needing the Create Expressions permission themselves.
    if !is_trusted_user(ctx) {
        let member = ctx
            .author_member()
            .await
            .ok_or_else(|| "I couldn't verify your server permissions.".to_string())?;

        let http = &ctx.serenity_context().http;

        let guild = guild_id
            .to_partial_guild(http)
            .await
            .map_err(|_| "I couldn't retrieve this server's information.".to_string())?;

        let channel = ctx
            .channel_id()
            .to_channel(http)
            .await
            .map_err(|_| "I couldn't retrieve this channel's information.".to_string())?;

        let serenity::Channel::Guild(channel) = channel else {
            return Err("I couldn't verify permissions in this channel.".to_string());
        };

        let permissions = guild.user_permissions_in(&channel, member.as_ref());

        if !permissions.contains(serenity::Permissions::CREATE_GUILD_EXPRESSIONS)
            && !permissions.contains(serenity::Permissions::ADMINISTRATOR)
        {
            return Err(
                "You need the **Create Expressions** permission in this server to add emotes here."
                    .to_string(),
            );
        }
    }

    validate_guild_target(ctx, guild_id).await
}

async fn fetch_image(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    if response
        .content_length()
        .is_some_and(|length| length > MAX_IMAGE_BYTES as u64)
    {
        return Err("Image exceeds the 256 KB upload limit.".to_string());
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Could not read image: {error}"))?
        .to_vec();

    if bytes.is_empty() {
        return Err("The downloaded image was empty.".to_string());
    }

    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("Image exceeds the 256 KB upload limit.".to_string());
    }

    Ok(bytes)
}

async fn download_discord_emote(
    client: &reqwest::Client,
    emote: &DiscordEmote,
) -> Result<DownloadedEmote, String> {
    let extension = if emote.animated { "gif" } else { "png" };
    let mime = if emote.animated {
        "image/gif"
    } else {
        "image/png"
    };

    let url = format!(
        "https://cdn.discordapp.com/emojis/{}.{}?size=128",
        emote.id, extension
    );

    let bytes = fetch_image(client, &url).await?;

    Ok(DownloadedEmote {
        name: sanitize_emoji_name(&emote.name),
        bytes,
        mime,
    })
}

async fn download_7tv(client: &reqwest::Client, input: &str) -> Result<DownloadedEmote, String> {
    static EMOTE_PATH: OnceLock<Regex> = OnceLock::new();

    let path_regex = EMOTE_PATH.get_or_init(|| Regex::new(r"(?i)/emotes/([a-z0-9]+)").unwrap());

    let emote_id = path_regex
        .captures(input)
        .and_then(|captures| captures.get(1))
        .map(|matched| matched.as_str())
        .unwrap_or(input.trim());

    if !emote_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Could not extract a valid 7TV emote ID.".to_string());
    }

    let api_url = format!("https://7tv.io/v3/emotes/{emote_id}");

    let response = client
        .get(&api_url)
        .send()
        .await
        .map_err(|error| format!("7TV request failed: {error}"))?;

    if !response.status().is_success() {
        return Err("🛑 Emote not found on 7TV.".to_string());
    }

    let data: Value = response
        .json()
        .await
        .map_err(|error| format!("Could not read 7TV response: {error}"))?;

    if data
        .get("state")
        .and_then(Value::as_array)
        .is_some_and(|states| {
            states
                .iter()
                .any(|state| state.as_str() == Some("NO_PERSONAL"))
        })
    {
        return Err(
            "🛑 This emote is marked as \"Denied Personal Use\" on 7TV and cannot be added."
                .to_string(),
        );
    }

    let name = data
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "7TV response did not contain an emote name.".to_string())?;

    let animated = data
        .get("animated")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let host_url = data
        .pointer("/host/url")
        .and_then(Value::as_str)
        .ok_or_else(|| "7TV response did not contain an image host.".to_string())?;

    let host_url = if host_url.starts_with("//") {
        format!("https:{host_url}")
    } else {
        host_url.to_string()
    };

    if !host_url.starts_with("https://") {
        return Err("7TV returned an invalid image host.".to_string());
    }

    let (extension, target_format, mime) = if animated {
        ("gif", "GIF", "image/gif")
    } else {
        ("png", "PNG", "image/png")
    };

    let mut best_name = format!("1x.{extension}");
    let mut best_size = 0_u64;

    if let Some(files) = data.pointer("/host/files").and_then(Value::as_array) {
        for file in files {
            let format = file.get("format").and_then(Value::as_str);
            let size = file.get("size").and_then(Value::as_u64);
            let filename = file.get("name").and_then(Value::as_str);

            if format == Some(target_format) {
                if let (Some(size), Some(filename)) = (size, filename) {
                    if size < MAX_IMAGE_BYTES as u64 && size > best_size {
                        best_size = size;
                        best_name = filename.to_string();
                    }
                }
            }
        }
    }

    let image_url = format!("{}/{}", host_url.trim_end_matches('/'), best_name);
    let bytes = fetch_image(client, &image_url).await?;

    Ok(DownloadedEmote {
        name: sanitize_emoji_name(name),
        bytes,
        mime,
    })
}

async fn download_emojigg(
    client: &reqwest::Client,
    input: &str,
) -> Result<DownloadedEmote, String> {
    let without_query = input.split('?').next().unwrap_or(input);

    let slug = without_query
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");

    if slug.is_empty()
        || !slug.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return Err("Could not extract a valid emoji.gg slug.".to_string());
    }

    let name = slug
        .split_once('-')
        .filter(|(prefix, _)| prefix.chars().all(|c| c.is_ascii_digit()))
        .map(|(_, remainder)| remainder)
        .unwrap_or(slug)
        .replace('-', "_");

    for (extension, mime) in [("png", "image/png"), ("gif", "image/gif")] {
        let url = format!("https://cdn3.emoji.gg/emojis/{slug}.{extension}");

        match fetch_image(client, &url).await {
            Ok(bytes) => {
                return Ok(DownloadedEmote {
                    name: sanitize_emoji_name(&name),
                    bytes,
                    mime,
                });
            }
            Err(error) if error.contains("HTTP 404") => continue,
            Err(error) => {
                tracing::warn!("emoji.gg download attempt failed: {error}");
            }
        }
    }

    Err("🛑 Could not find the emoji on emoji.gg (tried .png and .gif).".to_string())
}

fn data_uri(emote: &DownloadedEmote) -> String {
    format!(
        "data:{};base64,{}",
        emote.mime,
        STANDARD.encode(&emote.bytes)
    )
}

async fn upload_emoji(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    emote: &DownloadedEmote,
) -> Result<String, String> {
    let image = data_uri(emote);

    guild_id
        .create_emoji(&ctx.serenity_context().http, &emote.name, &image)
        .await
        .map(|created| {
            let prefix = if created.animated { "a" } else { "" };
            format!("<{prefix}:{}:{}>", created.name, created.id)
        })
        .map_err(|error| error.to_string())
}

/// Add an emoji from 7TV, emoji.gg, or Discord.
///
/// Multiple Discord emoji can be supplied, separated by spaces or commas.
/// Leave the destination empty to add the emoji to the current server.
/// Configured storage servers are available to trusted users.
#[poise::command(slash_command)]
pub async fn emote(
    ctx: Context<'_>,

    #[description = "7TV link/slug, emoji.gg link/slug, or Discord emoji"] url: String,

    #[description = "Storage server (leave empty to use this server)"]
    #[autocomplete = "storage_guild_autocomplete"]
    target_server: Option<String>,
) -> Result<(), Error> {
    let discord_emotes = match parse_discord_emotes(&url) {
        Ok(result) => result,
        Err(error) => {
            ctx.send(poise::CreateReply::default().content(error).ephemeral(true))
                .await?;
            return Ok(());
        }
    };

    let source = if discord_emotes.is_some() {
        "discord"
    } else if url.to_ascii_lowercase().contains("7tv.io")
        || url.to_ascii_lowercase().contains("7tv.app")
        || url.to_ascii_lowercase().contains("7tv.com")
        || url.to_ascii_lowercase().contains("7tv.gg")
        || (url.len() >= 4 && url.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        "7tv"
    } else if url.to_ascii_lowercase().contains("emoji.gg") || {
        let slug = url.trim();
        let first = slug.split('-').next().unwrap_or("");
        first.chars().all(|c| c.is_ascii_digit()) && !first.is_empty() && slug.contains('-')
    } {
        "emojigg"
    } else {
        ctx.send(
            poise::CreateReply::default()
                .content(
                    "Unrecognised input. Provide a 7TV emote link/slug, an emoji.gg link/slug, or Discord emoji (`<:name:id>`).",
                )
                .ephemeral(true),
        )
        .await?;
        return Ok(());
    };

    // An explicitly selected storage server is only available to trusted users.
    let target = match target_server {
        Some(raw_id) => {
            if !is_trusted(ctx).await {
                ctx.send(unauthorized()).await?;
                return Ok(());
            }

            validate_storage_target(ctx, &raw_id).await
        }
        None => validate_current_target(ctx).await,
    };

    let mut target = match target {
        Ok(target) => target,
        Err(error) => {
            ctx.send(
                poise::CreateReply::default()
                    .content(format!("🛑 {error}"))
                    .ephemeral(true),
            )
            .await?;
            return Ok(());
        }
    };

    ctx.defer_ephemeral().await?;

    let client = &ctx.data().http;

    // Bulk Discord emoji import: attempt every emoji independently.
    if let Some(emotes) = discord_emotes {
        let attempted = emotes.len();
        let mut successes = Vec::new();
        let mut failures = Vec::new();

        for emote in &emotes {
            let label = format!("{} ({})", emote.name, emote.id);

            // Static and animated emoji have separate capacity pools.
            if target.remaining_slots(emote.animated) == 0 {
                let emoji_type = if emote.animated { "animated" } else { "static" };

                failures.push(format!(
                    "❌ `{label}`: **{}** has no remaining {emoji_type} emoji slots.",
                    target.name
                ));
                continue;
            }

            match download_discord_emote(client, emote).await {
                Ok(downloaded) => match upload_emoji(ctx, target.id, &downloaded).await {
                    Ok(created) => {
                        target.consume_slot(emote.animated);

                        tracing::info!(
                            "Added Discord emoji: user={} guild={} name={} source=discord animated={}",
                            ctx.author().id,
                            target.id,
                            downloaded.name,
                            emote.animated
                        );

                        successes.push(format!("✅ {created}"));
                    }
                    Err(error) => {
                        failures.push(format!("❌ `{label}`: {error}"));
                    }
                },
                Err(error) => {
                    failures.push(format!("❌ `{label}`: {error}"));
                }
            }
        }

        let mut report = format!(
            "Finished importing **{attempted}** Discord emoji into **{}**.\n",
            target.name
        );

        if !successes.is_empty() {
            report.push_str("\n**Added:**\n");
            report.push_str(&successes.join("\n"));
        }

        if !failures.is_empty() {
            report.push_str("\n\n**Failed:**\n");
            report.push_str(&failures.join("\n"));
        }

        tracing::info!(
            "Discord emoji batch completed: user={} guild={} attempted={} succeeded={} failed={}",
            ctx.author().id,
            target.id,
            attempted,
            successes.len(),
            failures.len()
        );

        if report.len() > 1900 {
            let mut end = 1850;

            while !report.is_char_boundary(end) {
                end -= 1;
            }

            report.truncate(end);
            report.push_str("\n… (some results omitted because the report was too long)");
        }

        ctx.say(report).await?;
        return Ok(());
    }

    // Single-emote 7TV or emoji.gg import.
    let downloaded = match source {
        "7tv" => download_7tv(client, &url).await,
        "emojigg" => download_emojigg(client, &url).await,
        _ => unreachable!(),
    };

    let downloaded = match downloaded {
        Ok(emote) => emote,
        Err(error) => {
            tracing::error!(
                "Emote download failed: source={} user={} error={}",
                source,
                ctx.author().id,
                error
            );

            ctx.say(format!("🛑 {error}")).await?;
            return Ok(());
        }
    };

    let is_animated = downloaded.mime == "image/gif";

    if target.remaining_slots(is_animated) == 0 {
        let emoji_type = if is_animated { "animated" } else { "static" };

        ctx.say(format!(
            "🛑 **{}** has no remaining {emoji_type} emoji slots.",
            target.name
        ))
        .await?;

        return Ok(());
    }

    match upload_emoji(ctx, target.id, &downloaded).await {
        Ok(created) => {
            target.consume_slot(is_animated);

            tracing::info!(
                "Added emoji: user={} guild={} source={} name={} animated={}",
                ctx.author().id,
                target.id,
                source,
                downloaded.name,
                is_animated
            );

            ctx.say(format!(
                "✅ Successfully added {created} to **{}**!",
                target.name
            ))
            .await?;
        }
        Err(error) => {
            tracing::error!(
                "Emoji upload failed: source={} user={} guild={} error={}",
                source,
                ctx.author().id,
                target.id,
                error
            );

            if error.to_ascii_lowercase().contains("403")
                || error.to_ascii_lowercase().contains("forbidden")
            {
                ctx.say(format!(
                    "🛑 I don't have permission to create emojis in **{}**.",
                    target.name
                ))
                .await?;
            } else {
                ctx.say(format!("🛑 Failed to create emoji: {error}"))
                    .await?;
            }
        }
    }

    Ok(())
}
