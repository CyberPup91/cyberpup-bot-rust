use crate::{Context, Error};
use poise::serenity_prelude as serenity;
use serenity::all::CreateEmbed;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;

#[derive(Clone, Debug)]
struct VoiceSettings {
    target_channel_id: Option<u64>,
    persistent: bool,
    deafen: bool,
    flaps: u8,
    last_disconnect: Option<Instant>,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            target_channel_id: None,
            persistent: true,
            deafen: true,
            flaps: 0,
            last_disconnect: None,
        }
    }
}

#[derive(Default)]
struct VoiceRuntime {
    settings: HashMap<u64, VoiceSettings>,
    reconnect_tasks: HashMap<u64, JoinHandle<()>>,
    stable_tasks: HashMap<u64, JoinHandle<()>>,
}

static VOICE_RUNTIME: OnceLock<Mutex<VoiceRuntime>> = OnceLock::new();

fn runtime() -> &'static Mutex<VoiceRuntime> {
    VOICE_RUNTIME.get_or_init(|| Mutex::new(VoiceRuntime::default()))
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

fn unauthorized() -> poise::CreateReply {
    poise::CreateReply::default()
        .content("🛑 Unauthorized.")
        .ephemeral(true)
}

/// Manage the bot's voice connection and voice channel status.
#[poise::command(slash_command, subcommands("join", "leave", "status", "setstatus"))]
pub async fn vc(_: Context<'_>) -> Result<(), Error> {
    Ok(())
}

/// Join a voice channel, with optional automatic recovery and self-deafening.
#[poise::command(slash_command)]
pub async fn join(
    ctx: Context<'_>,
    #[description = "Voice channel (defaults to the channel you are in)"]
    #[channel_types("Voice")]
    channel: Option<serenity::ChannelId>,
    #[description = "Automatically rejoin after an unexpected disconnect"] persistent: Option<bool>,
    #[description = "Self-deafen after joining"] deafen: Option<bool>,
) -> Result<(), Error> {
    if !is_trusted(ctx).await {
        ctx.send(unauthorized()).await?;
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    let guild_id = ctx
        .guild_id()
        .ok_or("This command only works in a server.")?;
    let target_channel = match channel {
        Some(id) => id,
        None => ctx
            .guild()
            .and_then(|guild| guild.voice_states.get(&ctx.author().id)?.channel_id)
            .ok_or("Please specify a voice channel or join one yourself first.")?,
    };
    let persistent = persistent.unwrap_or(true);
    let deafen = deafen.unwrap_or(true);
    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client is not initialized.")?;
    let call = match manager.join(guild_id, target_channel).await {
        Ok(call) => call,
        Err(error) => {
            ctx.say(format!("❌ Failed to join voice channel: {error}"))
                .await?;
            return Ok(());
        }
    };
    if let Err(error) = call.lock().await.deafen(deafen).await {
        ctx.say(format!("❌ Joined, but could not set self-deafen: {error}"))
            .await?;
        return Ok(());
    }
    stop_reconnect(guild_id.get()).await;

    {
        let mut runtime = runtime().lock().await;
        runtime.settings.insert(
            guild_id.get(),
            VoiceSettings {
                target_channel_id: Some(target_channel.get()),
                persistent,
                deafen,
                ..VoiceSettings::default()
            },
        );
    }
    mark_connected(ctx.serenity_context().clone(), guild_id.get()).await;

    let persistent_label = if persistent {
        "🔁 Persistent mode **enabled**."
    } else {
        "⏹️ Persistent mode **disabled**."
    };
    let deafen_label = if deafen {
        "🔇 Self-deafened **enabled**."
    } else {
        "🔊 Self-deafened **disabled**."
    };
    let embed = CreateEmbed::new()
        .title("🔊 Joined Voice Channel")
        .description(format!(
            "Connected to <#{target_channel}>.\n\n{persistent_label}\n{deafen_label}"
        ))
        .color(0x57F287)
        .footer(serenity::CreateEmbedFooter::new(format!(
            "Requested by {}",
            ctx.author().display_name()
        )));
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

/// Leave voice and disable automatic reconnection for this session.
#[poise::command(slash_command)]
pub async fn leave(ctx: Context<'_>) -> Result<(), Error> {
    if !is_trusted(ctx).await {
        ctx.send(unauthorized()).await?;
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    let Some(guild_id) = ctx.guild_id() else {
        ctx.say("This command only works in a server.").await?;
        return Ok(());
    };
    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client is not initialized.")?;
    {
        let mut runtime = runtime().lock().await;
        let state = runtime.settings.entry(guild_id.get()).or_default();
        state.persistent = false;
        state.target_channel_id = None;
    }
    stop_reconnect(guild_id.get()).await;
    stop_stability_watchdog(guild_id.get()).await;
    let call = manager.get(guild_id);
    let channel_name = if let Some(call) = &call {
        let call = call.lock().await;
        call.current_channel()
            .map(|channel| format!("<#{channel}>"))
            .unwrap_or_else(|| "the voice channel".to_owned())
    } else {
        ctx.say("ℹ️ I'm not currently in any voice channel.")
            .await?;
        return Ok(());
    };

    if let Err(error) = manager.remove(guild_id).await {
        ctx.say(format!("❌ Could not disconnect cleanly: {error}"))
            .await?;
        return Ok(());
    }
    ctx.say(format!(
        "🔇 Disconnected from {channel_name}. Persistent mode is disabled for this session."
    ))
    .await?;
    Ok(())
}

/// Show the current connection, target, persistence, and self-deafen settings.
#[poise::command(slash_command)]
pub async fn status(ctx: Context<'_>) -> Result<(), Error> {
    if !is_trusted(ctx).await {
        ctx.send(unauthorized()).await?;
        return Ok(());
    }
    let guild_id = ctx
        .guild_id()
        .ok_or("This command only works in a server.")?;
    let settings = {
        let mut runtime = runtime().lock().await;
        runtime.settings.entry(guild_id.get()).or_default().clone()
    };
    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird voice client is not initialized.")?;
    let connected_channel = if let Some(call) = manager.get(guild_id) {
        call.lock()
            .await
            .current_channel()
            .map(|channel| channel.0.get())
    } else {
        None
    };
    let connection = connected_channel
        .map(|id| format!("✅ Connected to <#{id}>"))
        .unwrap_or_else(|| "❌ Not currently in a voice channel".to_owned());
    let persistent = if settings.persistent {
        "🔁 **Enabled** — the bot will try to rejoin if disconnected"
    } else {
        "⏹️ **Disabled** — the bot will not automatically rejoin"
    };
    let deafen = if settings.deafen {
        "🔇 **Enabled** — self-deafened"
    } else {
        "🔊 **Disabled** — audio is not self-deafened"
    };
    let target = settings
        .target_channel_id
        .map(|id| format!("📌 <#{id}>"))
        .unwrap_or_else(|| "📌 No target channel set".to_owned());
    let embed = CreateEmbed::new()
        .title("🔊 Voice Status")
        .color(0x5865F2)
        .field("Connection", connection, false)
        .field("Persistent Mode", persistent, false)
        .field("Self-Deafen", deafen, false)
        .field("Target", target, false);
    ctx.send(poise::CreateReply::default().embed(embed).ephemeral(true))
        .await?;
    Ok(())
}

/// Set or clear a voice channel's status text.
#[poise::command(slash_command)]
pub async fn setstatus(
    ctx: Context<'_>,
    #[description = "Voice channel to update"]
    #[channel_types("Voice")]
    channel: serenity::ChannelId,
    #[description = "Status text; leave empty to clear"] status: Option<String>,
) -> Result<(), Error> {
    if !is_trusted(ctx).await {
        ctx.send(unauthorized()).await?;
        return Ok(());
    }
    ctx.defer_ephemeral().await?;
    let body = serde_json::json!({ "status": status });
    let url = format!(
        "https://discord.com/api/v10/channels/{}/voice-status",
        channel.get()
    );
    let response = reqwest::Client::new()
        .put(url)
        .header("Authorization", ctx.http().token())
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    if !response.status().is_success() {
        let error = response.text().await.unwrap_or_default();
        ctx.say(format!("❌ Failed to update voice status: {error}"))
            .await?;
        return Ok(());
    }
    let action = if status.as_deref().is_some_and(|text| !text.is_empty()) {
        format!("set to **{}**", status.as_deref().unwrap_or_default())
    } else {
        "cleared".to_owned()
    };
    ctx.say(format!(
        "✅ Voice channel <#{channel}> status has been {action}."
    ))
    .await?;
    Ok(())
}

/// React to unexpected bot voice-state changes and recover persistent sessions.
pub async fn handle_voice_state_update(
    ctx: &serenity::Context,
    old: Option<&serenity::VoiceState>,
    new: &serenity::VoiceState,
) -> Result<(), Error> {
    if new.user_id != ctx.cache.current_user().id {
        return Ok(());
    }
    let Some(guild_id) = new.guild_id else {
        return Ok(());
    };
    let settings = {
        let runtime = runtime().lock().await;
        runtime.settings.get(&guild_id.get()).cloned()
    };
    let Some(settings) =
        settings.filter(|state| state.persistent && state.target_channel_id.is_some())
    else {
        return Ok(());
    };

    let left_channel = old.and_then(|state| state.channel_id).is_some() && new.channel_id.is_none();
    if left_channel {
        let manager = songbird::get(ctx)
            .await
            .ok_or("Songbird voice client is not initialized")?;
        // Songbird performs its own WebSocket/session recovery while its Call
        // remains active; only schedule a full rejoin once that Call is gone.
        let native_reconnect_pending = manager.get(guild_id).is_some_and(|call| {
            let call = call.try_lock();
            call.is_err() || call.is_ok_and(|call| call.current_channel().is_some())
        });
        if native_reconnect_pending {
            return Ok(());
        }
        tracing::warn!(
            guild_id = guild_id.get(),
            "Voice state detached; scheduling recovery"
        );
        start_reconnect(ctx.clone(), guild_id.get()).await;
        return Ok(());
    }

    if let Some(channel_id) = new.channel_id {
        if settings.target_channel_id != Some(channel_id.get()) {
            tracing::warn!(guild_id = guild_id.get(), actual = channel_id.get(), target = ?settings.target_channel_id, "Bot moved away from persistent voice target");
            start_reconnect(ctx.clone(), guild_id.get()).await;
        }
    }
    Ok(())
}

async fn stop_reconnect(guild_id: u64) {
    if let Some(task) = runtime().lock().await.reconnect_tasks.remove(&guild_id) {
        task.abort();
    }
}

async fn stop_stability_watchdog(guild_id: u64) {
    if let Some(task) = runtime().lock().await.stable_tasks.remove(&guild_id) {
        task.abort();
    }
}

async fn start_reconnect(ctx: serenity::Context, guild_id: u64) {
    let mut runtime = runtime().lock().await;
    if runtime
        .reconnect_tasks
        .get(&guild_id)
        .is_some_and(|task| !task.is_finished())
    {
        return;
    }
    let task = tokio::spawn(async move { reconnect_loop(ctx, guild_id).await });
    runtime.reconnect_tasks.insert(guild_id, task);
}

async fn reconnect_loop(ctx: serenity::Context, guild_id: u64) {
    let (target, settle_delay, flap_count) = {
        let mut runtime = runtime().lock().await;
        let (target, flap_count) = {
            let Some(settings) = runtime.settings.get_mut(&guild_id) else {
                return;
            };
            if !settings.persistent {
                return;
            }
            let now = Instant::now();
            if settings
                .last_disconnect
                .is_some_and(|last| now.duration_since(last) < Duration::from_secs(60))
            {
                settings.flaps = settings.flaps.saturating_add(1).min(4);
            } else {
                settings.flaps = 0;
            }
            settings.last_disconnect = Some(now);
            (settings.target_channel_id, settings.flaps)
        };
        if let Some(task) = runtime.stable_tasks.remove(&guild_id) {
            task.abort();
        }
        let delays = [2_u64, 5, 10, 20, 30];
        let delay = delays[flap_count as usize];
        (target, Duration::from_secs(delay), flap_count)
    };
    let Some(target) = target else {
        return;
    };
    tracing::warn!(
        guild_id,
        flap_count,
        delay = settle_delay.as_secs(),
        "Starting voice recovery"
    );
    tokio::time::sleep(settle_delay).await;

    let target_channel = ctx
        .cache
        .guild(serenity::GuildId::new(guild_id))
        .and_then(|guild| {
            guild
                .channels
                .get(&serenity::ChannelId::new(target))
                .cloned()
        });
    if !target_channel.is_some_and(|channel| channel.kind == serenity::ChannelType::Voice) {
        tracing::error!(
            guild_id,
            channel_id = target,
            "Persistent voice target is missing or is not a voice channel; disabling recovery"
        );
        disable_persistence(guild_id).await;
        return;
    }

    let manager = match songbird::get(&ctx).await {
        Some(manager) => manager,
        None => {
            tracing::error!(
                guild_id,
                "Songbird manager is unavailable during voice recovery"
            );
            disable_persistence(guild_id).await;
            return;
        }
    };
    let mut delay = Duration::from_secs(2);
    for attempt in 1..=50 {
        let settings = {
            let runtime = runtime().lock().await;
            runtime.settings.get(&guild_id).cloned()
        };
        let Some(settings) =
            settings.filter(|state| state.persistent && state.target_channel_id == Some(target))
        else {
            break;
        };
        let guild = serenity::GuildId::new(guild_id);
        let channel_id = serenity::ChannelId::new(target);
        let connected_to_target = if let Some(call) = manager.get(guild) {
            call.lock()
                .await
                .current_channel()
                .is_some_and(|id| id.0.get() == target)
        } else {
            false
        };
        let joined = if connected_to_target {
            true
        } else {
            match manager.join(guild, channel_id).await {
                Ok(call) => match call.lock().await.deafen(settings.deafen).await {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(guild_id, attempt, %error, "Could not reapply voice deafen setting");
                        false
                    }
                },
                Err(error) => {
                    tracing::warn!(guild_id, attempt, %error, "Voice reconnect attempt failed");
                    false
                }
            }
        };
        if joined {
            tracing::info!(
                guild_id,
                channel_id = target,
                attempt,
                "Voice connection recovered"
            );
            mark_connected(ctx.clone(), guild_id).await;
            return;
        }
        if attempt == 50 {
            tracing::error!(guild_id, "Giving up voice recovery after 50 attempts");
            disable_persistence(guild_id).await;
            return;
        }
        let jitter_ms = (rand::random::<u16>() % 500) as u64;
        tokio::time::sleep(delay + Duration::from_millis(jitter_ms)).await;
        delay = std::cmp::min(delay.mul_f64(1.5), Duration::from_secs(30));
    }
}

async fn disable_persistence(guild_id: u64) {
    if let Some(settings) = runtime().lock().await.settings.get_mut(&guild_id) {
        settings.target_channel_id = None;
        settings.persistent = false;
    }
}

async fn mark_connected(ctx: serenity::Context, guild_id: u64) {
    stop_stability_watchdog(guild_id).await;
    let task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let manager = songbird::get(&ctx).await;
        let target = runtime()
            .lock()
            .await
            .settings
            .get(&guild_id)
            .filter(|settings| settings.persistent)
            .and_then(|settings| settings.target_channel_id);
        let healthy = match (manager, target) {
            (Some(manager), Some(target)) => manager
                .get(serenity::GuildId::new(guild_id))
                .is_some_and(|call| {
                    call.try_lock().is_ok_and(|call| {
                        call.current_channel()
                            .is_some_and(|id| id.0.get() == target)
                    })
                }),
            _ => false,
        };
        if healthy {
            if let Some(settings) = runtime().lock().await.settings.get_mut(&guild_id) {
                settings.flaps = 0;
            }
            tracing::info!(
                guild_id,
                "Voice session remained stable; flap counter reset"
            );
        }
    });
    runtime().lock().await.stable_tasks.insert(guild_id, task);
}
