use crate::{Context, Error};
use chrono::Utc;
use poise::serenity_prelude as serenity;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum PurgeFilterChoice {
    #[name = "All messages"]
    All,
    #[name = "Embeds"]
    Embeds,
    #[name = "Files/Attachments"]
    Files,
    #[name = "Images"]
    Images,
    #[name = "User"]
    User,
    #[name = "Contains Text"]
    Contains,
    #[name = "Links"]
    Links,
    #[name = "Mentions"]
    Mentions,
    #[name = "Bot Users"]
    Bot,
    #[name = "Human Users"]
    Human,
    #[name = "Custom Emojis"]
    CustomEmojis,
    #[name = "Clear Reactions"]
    Reactions,
}

/// Export this server's role and channel IDs to a text file.
#[poise::command(slash_command, rename = "dump-ids")]
pub async fn dump_ids(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild = ctx.partial_guild().await.ok_or("Must be in a guild.")?;

    let mut output = String::from("=== SERVER ROLES ===\n");
    for role in guild.roles.values() {
        output.push_str(&format!("{}: {}\n", role.name, role.id));
    }

    output.push_str("\n=== SERVER CHANNELS ===\n");
    let channels = guild.channels(&ctx.http()).await?;
    for channel in channels.values() {
        output.push_str(&format!("{}: {}\n", channel.name, channel.id));
    }

    let attachment = serenity::CreateAttachment::bytes(output.into_bytes(), "server_ids.txt");
    ctx.send(
        poise::CreateReply::default()
            .content("Here is the complete list of all Role and Channel IDs:")
            .attachment(attachment)
            .ephemeral(true),
    )
    .await?;

    Ok(())
}

/// Export the bot application's custom emoji markup.
#[poise::command(slash_command, rename = "dump-emojis")]
pub async fn dump_emojis(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let bot_id = ctx.framework().bot_id();
    let url = format!("https://discord.com/api/v10/applications/{bot_id}/emojis");

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", ctx.http().token())
        .send()
        .await?;

    if !response.status().is_success() {
        ctx.say("🛑 Failed to fetch Application Emojis from Discord API.")
            .await?;
        return Ok(());
    }

    let json: serde_json::Value = response.json().await?;
    let items = json.get("items").and_then(|v| v.as_array());

    let mut lines = Vec::new();
    if let Some(emojis) = items {
        for e in emojis {
            let name = e.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let id = e.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let anim = if e.get("animated").and_then(|v| v.as_bool()).unwrap_or(false) {
                "a"
            } else {
                ""
            };
            lines.push(format!("<{anim}:{name}:{id}>"));
        }
    }

    if lines.is_empty() {
        ctx.say("No Application Emojis found on the Developer Portal.")
            .await?;
        return Ok(());
    }

    let content = lines.join("\n");
    let attachment = serenity::CreateAttachment::bytes(content.into_bytes(), "app_emojis.txt");

    ctx.send(
        poise::CreateReply::default()
            .content("Here are your formatted Application Emojis:")
            .attachment(attachment)
            .ephemeral(true),
    )
    .await?;

    Ok(())
}

/// Set the bot's global presence to invisible.
#[poise::command(slash_command, rename = "toggle-status")]
pub async fn toggle_status(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    ctx.serenity_context()
        .set_presence(None, serenity::OnlineStatus::Invisible);

    ctx.say("Bot status set to **Invisible** (appears offline).")
        .await?;
    Ok(())
}

const EMOJI_PATTERN: &str = r"<a?:[a-zA-Z0-9_]+:[0-9]+>";

fn check_custom_emoji(content: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(EMOJI_PATTERN).unwrap());
    re.is_match(content)
}

fn message_snowflake_timestamp(msg_id: u64) -> u64 {
    ((msg_id >> 22) + 1420070400000) / 1000
}

fn message_unix_time(msg: &serenity::Message) -> i64 {
    let ts = message_snowflake_timestamp(msg.id.get());
    (ts / 1000) as i64
}

fn get_display_name(message: &serenity::Message) -> String {
    message
        .member
        .as_ref()
        .and_then(|m| m.nick.as_deref())
        .or(message.author.global_name.as_deref())
        .unwrap_or(&message.author.name)
        .to_string()
}

fn check_message_filter(
    msg: &serenity::Message,
    filter: &PurgeFilterChoice,
    target_member_id: Option<u64>,
    target_username: Option<&str>,
    substring: Option<&str>,
    prefix: Option<&str>,
    exclude_pinned: bool,
) -> bool {
    if exclude_pinned && msg.pinned {
        return false;
    }

    if let Some(target_id) = target_member_id {
        if msg.author.id.get() != target_id {
            return false;
        }
    }
    if let Some(tu) = target_username {
        let name_match = msg.author.name.to_lowercase() == tu;
        let display_match = get_display_name(msg).to_lowercase() == tu;
        let mention_match = format!("{}", msg.author).to_lowercase() == tu;
        if !name_match && !display_match && !mention_match {
            return false;
        }
    }

    if let Some(sub) = substring {
        if !msg.content.to_lowercase().contains(&sub.to_lowercase()) {
            return false;
        }
    }

    if filter != &PurgeFilterChoice::All {
        match filter {
            PurgeFilterChoice::Embeds => !msg.embeds.is_empty(),
            PurgeFilterChoice::Files => !msg.attachments.is_empty(),
            PurgeFilterChoice::Images => {
                if msg.attachments.iter().any(|a| {
                    a.content_type
                        .as_ref()
                        .is_some_and(|ct| ct.starts_with("image/"))
                }) {
                    return true;
                }
                msg.embeds.iter().any(|e| {
                    e.image.is_some()
                        || e.thumbnail.is_some()
                })
            }
            PurgeFilterChoice::User => target_member_id.is_none(),
            PurgeFilterChoice::Contains => substring.is_some(),
            PurgeFilterChoice::Links => {
                msg.content.contains("http://") || msg.content.contains("https://")
            }
            PurgeFilterChoice::Mentions => {
                !msg.mentions.is_empty() || !msg.mention_roles.is_empty()
            }
            PurgeFilterChoice::Bot => {
                if msg.author.bot {
                    true
                } else {
                    prefix.map_or(false, |p| msg.content.starts_with(p))
                }
            }
            PurgeFilterChoice::Human => !msg.author.bot,
            PurgeFilterChoice::CustomEmojis => check_custom_emoji(&msg.content),
            PurgeFilterChoice::Reactions => !msg.reactions.is_empty(),
            PurgeFilterChoice::All => true,
        }
    } else {
        true
    }
}

fn parse_message_id(input: &str) -> Option<u64> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Ok(id) = input.parse::<u64>() {
        return Some(id);
    }
    let patterns = [
        r"https?://(?:ptb\.|canary\.)?discord(?:app)?\.com/channels/\d+/\d+/(\d+)",
        r"https?://(?:ptb\.|canary\.)?discord(?:app)?\.com/channels/(@me|@\d+)/(\d+)/(\d+)",
    ];
    for pattern in &patterns {
        if let Ok(re) = regex::Regex::new(pattern) {
            if let Some(caps) = re.captures(input) {
                if let Some(id_str) = caps.get(3).or_else(|| caps.get(2)) {
                    if let Ok(id) = id_str.as_str().parse::<u64>() {
                        return Some(id);
                    }
                }
            }
        }
    }
    None
}

/// Delete or simulate deleting recent messages using optional filters.
#[poise::command(slash_command)]
pub async fn purge(
    ctx: Context<'_>,
    #[description = "Number of messages to search backwards"] count: u64,
    #[description = "Filter type"] filter_type: Option<PurgeFilterChoice>,
    #[description = "User to purge (ID, mention, username, or display name)"] member: Option<String>,
    #[description = "Text substring filter"] substring: Option<String>,
    #[description = "Command prefix (for 'Bot Users' filter)"] prefix: Option<String>,
    #[description = "Protect pinned messages (default: true)"] exclude_pinned: Option<bool>,
    #[description = "Also purge messages from active and archived threads"] purge_threads: Option<bool>,
    #[description = "Only purge messages before this message link/ID"] before_message: Option<String>,
    #[description = "Only purge messages after this message link/ID"] after_message: Option<String>,
    #[description = "Simulate only (dry run)"] simulate: Option<bool>,
    #[description = "Use bulk delete (messages under 14 days old, default: true)"] bulk: Option<bool>,
    #[description = "Delay in seconds between individual deletes (default: 1.5)"] delay: Option<f64>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;

    let channel_id = ctx.channel_id();
    let guild_id = ctx.guild_id().ok_or("Must be in a guild.")?;
    let is_simulation = simulate.unwrap_or(false);
    let do_bulk = bulk.unwrap_or(true);
    let delete_delay = delay.unwrap_or(1.5);
    let do_exclude_pinned = exclude_pinned.unwrap_or(true);
    let do_purge_threads = purge_threads.unwrap_or(false);
    let filter = filter_type.unwrap_or(PurgeFilterChoice::All);
    let prefix_str = prefix.as_deref();
    let substring_str = substring.as_deref();

    let before_msg_id = parse_message_id(before_message.as_deref().unwrap_or(""));
    let after_msg_id = parse_message_id(after_message.as_deref().unwrap_or(""));

    let mut target_member_id: Option<u64> = None;
    let mut target_username: Option<String> = None;
    if let Some(member_str) = &member {
        let id_re = regex::Regex::new(r"\d+").unwrap();
        if let Some(id_match) = id_re.captures(member_str) {
            if let Some(id_cap) = id_match.get(0) {
                let id_str = id_cap.as_str();
                if id_str.len() >= 15 {
                    if let Ok(id) = id_str.parse::<u64>() {
                        target_member_id = Some(id);
                    }
                }
            }
        }
        if target_member_id.is_none() {
            target_username = Some(member_str.to_lowercase());
        }
    }

    let now_utc = Utc::now();
    let fourteen_days_secs = 14 * 24 * 60 * 60;
    let mut last_status_time = now_utc.timestamp();

    let mut to_delete: Vec<serenity::Message> = Vec::new();
    let mut total_scanned: u64 = 0;
    let mut total_matched: u64 = 0;
    let mut skipped_pinned: u64 = 0;
    let mut failed_count: u64 = 0;
    let mut user_breakdown: HashMap<String, u64> = HashMap::new();

    // Scan main channel
    let mut history = serenity::GetMessages::new()
        .limit(count.min(100) as u8);
    if let Some(before_id) = before_msg_id {
        history = history.before(serenity::MessageId::new(before_id));
    }
    if let Some(after_id) = after_msg_id {
        history = history.after(serenity::MessageId::new(after_id));
    }

    let messages = channel_id.messages(&ctx.http(), history).await?;

    for msg in messages {
        total_scanned += 1;
        let msg_id = msg.id.get();

        if let Some(before_id) = before_msg_id {
            if msg_id >= before_id {
                continue;
            }
        }
        if let Some(after_id) = after_msg_id {
            if msg_id <= after_id {
                continue;
            }
        }

        if do_exclude_pinned && msg.pinned {
            skipped_pinned += 1;
            continue;
        }

        if check_message_filter(
            &msg,
            &filter,
            target_member_id,
            target_username.as_deref(),
            substring_str,
            prefix_str,
            do_exclude_pinned,
        ) {
            to_delete.push(msg);
            total_matched += 1;
        }

        let now_ts = now_utc.timestamp();
        if now_ts - last_status_time >= 3 || total_scanned % 50 == 0 {
            let mut status = format!(
                "🧹 Scanning... Scanned: {}, Matched: {}",
                total_scanned, total_matched
            );
            if skipped_pinned > 0 {
                status.push_str(&format!(", Skipped (Pinned): {}", skipped_pinned));
            }
            let _ = channel_id
                .send_message(&ctx.http(), serenity::CreateMessage::new().content(status))
                .await;
            last_status_time = now_ts;
        }
    }

    // Collect threads if requested
    let mut targets = vec![channel_id];

    if do_purge_threads {
        // Active threads via PartialGuild
        if let Ok(partial_guild) = guild_id.to_partial_guild(&ctx.http()).await {
            if let Ok(active_data) = partial_guild.get_active_threads(&ctx.http()).await {
                for thread in active_data.threads {
                    targets.push(thread.id);
                }
            }
        }

        // Archived public threads - all threads in response are already archived
        if let Ok(response) = ctx
            .http()
            .get_channel_archived_public_threads(channel_id, None, Some(50))
            .await
        {
            for thread in response.threads {
                targets.push(thread.id);
            }
        }

        // Archived private threads - all threads in response are already archived
        if let Ok(response) = ctx
            .http()
            .get_channel_archived_private_threads(channel_id, None, Some(50))
            .await
        {
            for thread in response.threads {
                targets.push(thread.id);
            }
        }
    }

    // Scan additional targets (threads)
    for target in &targets {
        let mut thread_query = serenity::GetMessages::new()
            .limit(count.min(100) as u8);
        if let Some(before_id) = before_msg_id {
            thread_query = thread_query.before(serenity::MessageId::new(before_id));
        }
        if let Some(after_id) = after_msg_id {
            thread_query = thread_query.after(serenity::MessageId::new(after_id));
        }

        let messages = target.messages(&ctx.http(), thread_query).await?;

        for msg in messages {
            total_scanned += 1;
            let msg_id = msg.id.get();

            if let Some(before_id) = before_msg_id {
                if msg_id >= before_id {
                    continue;
                }
            }
            if let Some(after_id) = after_msg_id {
                if msg_id <= after_id {
                    continue;
                }
            }

            if do_exclude_pinned && msg.pinned {
                skipped_pinned += 1;
                continue;
            }

            if check_message_filter(
                &msg,
                &filter,
                target_member_id,
                target_username.as_deref(),
                substring_str,
                prefix_str,
                do_exclude_pinned,
            ) {
                to_delete.push(msg);
                total_matched += 1;
            }

            let now_ts = now_utc.timestamp();
            if now_ts - last_status_time >= 3 || total_scanned % 50 == 0 {
                let mut status = format!(
                    "🧹 Scanning... Scanned: {}, Matched: {}",
                    total_scanned, total_matched
                );
                if skipped_pinned > 0 {
                    status.push_str(&format!(", Skipped (Pinned): {}", skipped_pinned));
                }
                let _ = channel_id
                    .send_message(&ctx.http(), serenity::CreateMessage::new().content(status))
                    .await;
                last_status_time = now_ts;
            }
        }
    }

    let mut deleted_count: u64 = 0;

    // Handle reaction clearing
    if filter == PurgeFilterChoice::Reactions {
        if !is_simulation {
            for msg in &to_delete {
                if msg.reactions.is_empty() {
                    continue;
                }
                match ctx
                    .http()
                    .delete_message_reactions(msg.channel_id, msg.id)
                    .await
                {
                    Ok(_) => {
                        deleted_count += 1;
                        user_breakdown
                            .entry(get_display_name(msg))
                            .and_modify(|c| *c += 1)
                            .or_insert(1);
                    }
                    Err(_) => {
                        failed_count += 1;
                    }
                }
            }
        } else {
            for msg in &to_delete {
                user_breakdown
                    .entry(get_display_name(msg))
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            }
        }

        let mut status = format!(
            "🧹 Cleared reactions from **{}** messages.",
            if is_simulation { to_delete.len() } else { deleted_count as usize }
        );
        if is_simulation {
            status.insert_str(0, "[SIMULATION] ");
        }
        if skipped_pinned > 0 {
            status.push_str(&format!(", Skipped (Pinned): {}", skipped_pinned));
        }
        if failed_count > 0 {
            status.push_str(&format!(", Failed: {}", failed_count));
        }
        ctx.say(status).await?;
        return Ok(());
    }

    // Handle message deletion

    if !is_simulation && !to_delete.is_empty() {
        let young_msgs: Vec<serenity::Message> = to_delete
            .iter()
            .filter(|m| (now_utc.timestamp() - message_unix_time(m)) < fourteen_days_secs)
            .cloned()
            .collect();

        let old_msgs: Vec<serenity::Message> = to_delete
            .iter()
            .filter(|m| (now_utc.timestamp() - message_unix_time(m)) >= fourteen_days_secs)
            .cloned()
            .collect();

        // Delete young messages (bulk or slow)
        if do_bulk && !young_msgs.is_empty() {
            for chunk in young_msgs.chunks(100) {
                let msg_ids: Vec<serenity::MessageId> = chunk.iter().map(|m| m.id).collect();

                if let Err(_) = channel_id.delete_messages(&ctx.http(), &msg_ids).await {
                    // Fallback to individual deletes
                    for msg in chunk {
                        if msg.delete(&ctx.http()).await.is_ok() {
                            deleted_count += 1;
                            user_breakdown
                                .entry(get_display_name(msg))
                                .and_modify(|c| *c += 1)
                                .or_insert(1);
                        } else {
                            failed_count += 1;
                        }
                        if delete_delay > 0.0 {
                            sleep(Duration::from_secs_f64(delete_delay)).await;
                        }
                    }
                } else {
                    deleted_count += chunk.len() as u64;
                    for msg in chunk {
                        user_breakdown
                            .entry(get_display_name(msg))
                            .and_modify(|c| *c += 1)
                            .or_insert(1);
                    }
                }
            }
        } else if !young_msgs.is_empty() {
            for msg in young_msgs {
                if msg.delete(&ctx.http()).await.is_ok() {
                    deleted_count += 1;
                    user_breakdown
                        .entry(get_display_name(&msg))
                        .and_modify(|c| *c += 1)
                        .or_insert(1);
                } else {
                    failed_count += 1;
                }
                if delete_delay > 0.0 {
                    sleep(Duration::from_secs_f64(delete_delay)).await;
                }
            }
        }

        // Delete old messages (always individual)
        for msg in old_msgs {
            if msg.delete(&ctx.http()).await.is_ok() {
                deleted_count += 1;
                user_breakdown
                    .entry(get_display_name(&msg))
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
            } else {
                failed_count += 1;
            }
            if delete_delay > 0.0 {
                sleep(Duration::from_secs_f64(delete_delay)).await;
            }
        }
    } else {
        deleted_count = to_delete.len() as u64;
        for msg in &to_delete {
            user_breakdown
                .entry(get_display_name(msg))
                .and_modify(|c| *c += 1)
                .or_insert(1);
        }
    }

    // Build summary report
    let mode_str = if is_simulation {
        "[SIMULATION] Would purge"
    } else {
        "Purged"
    };

    let mut report = format!(
        "🧹 **{} {} messages** matching filter criteria.",
        mode_str, to_delete.len()
    );
    report.push_str(&format!("\nScanned: {}", total_scanned));
    report.push_str(&format!("\nMatched: {}", total_matched));
    if !is_simulation {
        report.push_str(&format!("\nDeleted: {}", deleted_count));
    }
    if skipped_pinned > 0 {
        report.push_str(&format!("\nSkipped (Pinned): {}", skipped_pinned));
    }
    if failed_count > 0 {
        report.push_str(&format!("\nFailed: {}", failed_count));
    }

    if !user_breakdown.is_empty() {
        report.push_str("\n\n**Top Contributors Affected:**");
        let mut sorted_users: Vec<(&String, &u64)> = user_breakdown.iter().collect();
        sorted_users.sort_by(|a, b| b.1.cmp(a.1));
        for (name, count) in sorted_users.iter().take(10) {
            report.push_str(&format!("\n- **{}**: {}", name, count));
        }
        if user_breakdown.len() > 10 {
            report.push_str(&format!(
                "\n...and {} more users.",
                user_breakdown.len() - 10
            ));
        }
    }

    ctx.say(report).await?;
    Ok(())
}

/// Sync the bot's slash commands to this server immediately.
#[poise::command(slash_command, rename = "sync")]
pub async fn sync_guild(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let guild_id = ctx.guild_id().ok_or("Must be in a guild.")?;

    poise::builtins::register_in_guild(ctx, &ctx.framework().options().commands, guild_id).await?;
    ctx.say(format!("✅ Commands synced to server ID `{guild_id}`!"))
        .await?;
    Ok(())
}

/// Run administrative utilities for this server.
#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "ADMINISTRATOR",
    subcommands("dump_ids", "dump_emojis", "toggle_status", "purge", "sync_guild")
)]
pub async fn admin(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}
