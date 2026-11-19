mod commands;

use poise::serenity_prelude as serenity;
use songbird::SerenityInit;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::Instant;

#[derive(Default)]
pub struct StickyRuntime {
    pub tasks: HashMap<u64, JoinHandle<()>>,
    pub last_stick_time: HashMap<u64, Instant>,
}

#[derive(Clone)]
pub struct Data {
    pub db: sqlx::SqlitePool,
    pub custom_commands_db: sqlx::SqlitePool,
    pub stats_db: sqlx::SqlitePool,
    pub greeters_db: sqlx::SqlitePool,
    pub tickets_db: sqlx::SqlitePool,
    pub counting_db: sqlx::SqlitePool,
    pub moderation_db: sqlx::SqlitePool,
    pub warnings_db: sqlx::SqlitePool,
    pub message_logger_db: sqlx::SqlitePool,
    pub reaction_roles_db: sqlx::SqlitePool,
    pub vents_db: sqlx::SqlitePool,
    pub builder_sessions: Arc<Mutex<HashMap<u64, commands::builder::BuilderDraft>>>,
    pub http: reqwest::Client,
    pub twitch: Arc<commands::creator_stats::api::TwitchApi>,
    pub sticky_channels: Arc<RwLock<HashSet<u64>>>,
    pub sticky_locks: Arc<RwLock<HashMap<u64, Arc<Mutex<()>>>>>,
    pub sticky_runtime: Arc<Mutex<StickyRuntime>>,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

async fn handle_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot } => {
            println!("Logged in as {}", data_about_bot.user.name);

            // Register user-installable personal application commands globally.
            let personal_commands = commands::personal();
            match poise::builtins::register_globally(&ctx.http, &personal_commands).await {
                Ok(_) => println!("[Commands] Personal application commands registered."),
                Err(error) => eprintln!("[Commands] Failed to register personal commands: {error}"),
            }

            let cfg = commands::whitelabel::load_config().await;
            let _ = commands::whitelabel::apply_saved_presence(ctx, &cfg).await;
        }

        serenity::FullEvent::GuildCreate { guild, is_new } => {
            let status = if is_new.unwrap_or(false) {
                "NEW"
            } else {
                "EXISTING"
            };

            println!(
                "⚡ Processing GuildCreate for [{}] {} ({})",
                status, guild.name, guild.id
            );

            match poise::builtins::register_in_guild(ctx, &commands::guild(), guild.id).await {
                Ok(_) => println!("✅ Commands synced to guild: {} ({})", guild.name, guild.id),
                Err(e) => eprintln!(
                    "[Sync Error] Failed to sync to {} ({}): {}",
                    guild.name, guild.id, e
                ),
            }
        }

        // Member joined the server.
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            if let Err(error) = commands::moderation::member_join(ctx, data, new_member).await {
                tracing::warn!("Sticky role restoration failed: {error}");
            }
            if new_member.pending {
                println!(
                    "👋 Member {} is pending screening; delaying welcome.",
                    new_member.user.name
                );
            } else {
                let guild_id = new_member.guild_id;
                let guild = ctx.cache.guild(guild_id).map(|g| (*g).clone());

                if let Some(guild) = guild {
                    if let Err(e) = commands::greeter::events::send_greeter(
                        ctx,
                        &data.greeters_db,
                        &guild,
                        &new_member.user,
                        true,
                    )
                    .await
                    {
                        eprintln!("❌ Failed to send welcome message: {e}");
                    }
                } else {
                    eprintln!("❌ Could not find guild {} in cache.", guild_id);
                }
            }
        }

        // Member completed Discord membership screening.
        serenity::FullEvent::GuildMemberUpdate {
            old_if_available,
            new,
            ..
        } => {
            if let Some(member) = new.as_ref() {
                if let Err(error) = commands::moderation::member_update(
                    ctx,
                    data,
                    old_if_available.as_ref(),
                    member,
                )
                .await
                {
                    tracing::warn!("Sticky role snapshot update failed: {error}");
                }
            }
            if let (Some(old_member), Some(new_member)) = (old_if_available.as_ref(), new.as_ref())
            {
                if old_member.pending && !new_member.pending {
                    let guild_id = new_member.guild_id;
                    let guild = ctx.cache.guild(guild_id).map(|g| (*g).clone());

                    if let Some(guild) = guild {
                        println!(
                            "✅ {} completed screening; sending welcome message.",
                            new_member.user.name
                        );

                        if let Err(e) = commands::greeter::events::send_greeter(
                            ctx,
                            &data.greeters_db,
                            &guild,
                            &new_member.user,
                            true,
                        )
                        .await
                        {
                            eprintln!("❌ Failed to send welcome after screening: {e}");
                        }
                    } else {
                        eprintln!("❌ Could not find guild {} in cache.", guild_id);
                    }
                }
            }
        }

        // Member left or was removed from the server.
        serenity::FullEvent::GuildMemberRemoval { guild_id, user, .. } => {
            if let Err(error) =
                commands::moderation::member_remove(ctx, data, *guild_id, user).await
            {
                tracing::warn!("Could not save sticky role snapshot: {error}");
            }
            let guild = { ctx.cache.guild(*guild_id).map(|g| (*g).clone()) };

            if let Some(guild) = guild {
                println!("👋 {} left {}.", user.name, guild.name);

                if let Err(e) = commands::greeter::events::send_greeter(
                    ctx,
                    &data.greeters_db,
                    &guild,
                    user,
                    false,
                )
                .await
                {
                    eprintln!("❌ Failed to send goodbye message: {e}");
                }
            } else {
                eprintln!(
                    "❌ Could not find guild {} in cache for goodbye message.",
                    guild_id
                );
            }
        }

        serenity::FullEvent::VoiceStateUpdate { old, new } => {
            if let Err(error) =
                commands::vc::handle_voice_state_update(ctx, old.as_ref(), new).await
            {
                tracing::warn!("Voice recovery event failed: {error}");
            }
        }

        serenity::FullEvent::Message { new_message } => {
            let Some(guild_id) = new_message.guild_id else {
                return Ok(());
            };

            if let Err(error) = commands::vents::handle_message(ctx, data, new_message).await {
                tracing::warn!("Vents panel sticky handler failed: {error}");
            }

            if let Err(error) = commands::message_logger::log_sent(ctx, data, new_message).await {
                tracing::warn!("Could not log message: {error}");
            }
            if let Err(error) =
                commands::custom_commands::handle_message(ctx, data, new_message).await
            {
                tracing::warn!("Custom command handler failed: {error}");
            }

            let channel_id = new_message.channel_id;
            let ch_id = channel_id.get();

            // Identify the dashboard's own embed so it never triggers
            // another repositioning cycle.
            let is_dashboard_message = new_message.embeds.iter().any(|embed| {
                embed
                    .title
                    .as_deref()
                    .map(|title| title.ends_with("Creator Stats"))
                    .unwrap_or(false)
            });

            // Our milestone alerts are bot messages, but they should still
            // trigger the dashboard sticky behavior.
            let is_milestone_alert =
                new_message.author.bot && new_message.content.contains("just hit a new milestone:");

            // Human messages and our own milestone announcements can
            // reposition the Creator Stats dashboard.
            if !is_dashboard_message && (!new_message.author.bot || is_milestone_alert) {
                commands::creator_stats::dashboard::handle_message(ctx, data, channel_id).await;
            }

            if !new_message.author.bot {
                if let Err(error) = commands::counting::handle_message(ctx, data, new_message).await
                {
                    tracing::error!("Counting message handler failed: {error}");
                }
            }

            // Keep the ordinary Sticky cog's existing bot-message behavior.
            if new_message.author.bot {
                return Ok(());
            }

            // Ordinary Sticky handling.
            let runtime_arc = data.sticky_runtime.clone();
            let mut runtime = runtime_arc.lock().await;

            if !data.sticky_channels.read().await.contains(&ch_id) {
                return Ok(());
            }

            if runtime.tasks.contains_key(&ch_id) {
                return Ok(());
            }

            let cooldown_remaining = runtime
                .last_stick_time
                .get(&ch_id)
                .map(|last| std::time::Duration::from_secs(4).saturating_sub(last.elapsed()))
                .unwrap_or_default();

            let delay = std::time::Duration::from_secs(3).max(cooldown_remaining);

            let ctx = ctx.clone();
            let db = data.db.clone();
            let custom_commands_db = data.custom_commands_db.clone();
            let stats_db = data.stats_db.clone();
            let greeters_db = data.greeters_db.clone();
            let tickets_db = data.tickets_db.clone();
            let counting_db = data.counting_db.clone();
            let moderation_db = data.moderation_db.clone();
            let warnings_db = data.warnings_db.clone();
            let message_logger_db = data.message_logger_db.clone();
            let reaction_roles_db = data.reaction_roles_db.clone();
            let vents_db = data.vents_db.clone();
            let builder_sessions = data.builder_sessions.clone();
            let http = data.http.clone();
            let twitch = data.twitch.clone();
            let sticky_chans = data.sticky_channels.clone();
            let locks = data.sticky_locks.clone();
            let sticky_runtime = runtime_arc.clone();

            let (start_tx, start_rx) = oneshot::channel();

            let task = tokio::spawn(async move {
                if start_rx.await.is_err() {
                    return;
                }

                tokio::time::sleep(delay).await;

                let dummy_data = Data {
                    db,
                    custom_commands_db,
                    stats_db,
                    greeters_db,
                    tickets_db,
                    counting_db,
                    moderation_db,
                    warnings_db,
                    message_logger_db,
                    reaction_roles_db,
                    vents_db,
                    builder_sessions,
                    http,
                    twitch,
                    sticky_channels: sticky_chans,
                    sticky_locks: locks,
                    sticky_runtime: sticky_runtime.clone(),
                };

                if let Err(error) = commands::sticky::stick_message(
                    &ctx,
                    &dummy_data,
                    guild_id.get() as i64,
                    channel_id,
                )
                .await
                {
                    eprintln!("[StickyPosts] Failed to repost sticky in channel {ch_id}: {error}");
                }

                let mut runtime = sticky_runtime.lock().await;

                runtime.last_stick_time.insert(ch_id, Instant::now());
                runtime.tasks.remove(&ch_id);
            });

            runtime.tasks.insert(ch_id, task);

            drop(runtime);

            let _ = start_tx.send(());
        }

        serenity::FullEvent::MessageUpdate {
            old_if_available,
            new,
            event,
        } => {
            if let Err(error) = commands::message_logger::log_edit(
                ctx,
                data,
                old_if_available.as_ref(),
                new.as_ref(),
                event,
            )
            .await
            {
                tracing::warn!("Could not log edited message: {error}");
            }
        }

        serenity::FullEvent::MessageDelete {
            channel_id,
            deleted_message_id,
            guild_id,
        } => {
            if let Err(error) =
                commands::reaction_roles::message_deleted(data, *deleted_message_id).await
            {
                tracing::warn!(
                    "Could not remove reaction-role configuration for deleted message: {error}"
                );
            }
            if let Some(guild_id) = guild_id {
                if let Err(error) = commands::message_logger::log_deleted_ids(
                    ctx,
                    data,
                    *guild_id,
                    *channel_id,
                    *deleted_message_id,
                )
                .await
                {
                    tracing::warn!("Could not log deleted message: {error}");
                }
            }
        }

        serenity::FullEvent::ReactionAdd { add_reaction } => {
            if let Err(error) =
                commands::reaction_roles::reaction_added(ctx, data, add_reaction).await
            {
                tracing::warn!("Could not assign reaction role: {error}");
            }
        }

        serenity::FullEvent::ReactionRemove { removed_reaction } => {
            if let Err(error) =
                commands::reaction_roles::reaction_removed(ctx, data, removed_reaction).await
            {
                tracing::warn!("Could not remove reaction role: {error}");
            }
        }

        serenity::FullEvent::ChannelDelete { channel, .. } => {
            let ch_id = channel.id.get();
            let guild_id = channel.guild_id.get() as i64;

            // Cancel pending sticky work and clear its cooldown state.
            {
                let mut runtime = data.sticky_runtime.lock().await;

                if let Some(task) = runtime.tasks.remove(&ch_id) {
                    task.abort();
                }

                runtime.last_stick_time.remove(&ch_id);
            }

            let _ = commands::sticky::delete_sticky_config(&data.db, guild_id, ch_id).await;

            data.sticky_channels.write().await.remove(&ch_id);
        }

        serenity::FullEvent::InteractionCreate {
            interaction: serenity::Interaction::Component(component),
        } => {
            if commands::custom_commands::handle_component(ctx, data, component).await? {
                return Ok(());
            }
            if commands::vents::handle_component(ctx, data, component).await? {
                return Ok(());
            }
            if commands::builder::handle_component(ctx, data, component).await? {
                return Ok(());
            }
            if commands::moderation::handle_component(ctx, data, component).await? {
                return Ok(());
            }
            if commands::warnings::handle_component(ctx, data, component).await? {
                return Ok(());
            }
            if commands::counting::handle_component(ctx, data, component).await? {
                return Ok(());
            }
            if let Err(error) =
                commands::tickets::interactions::handle_component(ctx, data, component).await
            {
                eprintln!("[Tickets] Component interaction failed: {error}");
                let _ = component
                    .create_response(
                        ctx,
                        serenity::CreateInteractionResponse::Message(
                            serenity::CreateInteractionResponseMessage::new()
                                .content("❌ The ticket action failed. Please try again or contact staff.")
                                .ephemeral(true),
                        ),
                    )
                    .await;
            }
        }

        serenity::FullEvent::InteractionCreate { interaction } => {
            if let Some(modal) = interaction.as_modal_submit() {
                commands::counting::handle_modal(ctx, data, modal).await?;
                commands::vents::handle_modal(ctx, data, modal).await?;
            }
        }

        serenity::FullEvent::ChannelCreate { channel } => {
            if let Err(error) = commands::warnings::channel_created(ctx, data, channel).await {
                tracing::warn!("Could not apply mute-role channel restrictions: {error}");
            }
        }

        _ => {}
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    dotenvy::dotenv().ok();

    let token = std::env::var("DISCORD_TOKEN").expect("Missing DISCORD_TOKEN in environment");

    tokio::fs::create_dir_all("/app/data").await?;

    // Main bot database.
    let db = sqlx::SqlitePool::connect("sqlite:///app/data/cyberpup_bot.db?mode=rwc").await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS startup_actions(action TEXT PRIMARY KEY,completed_at INTEGER NOT NULL)")
        .execute(&db)
        .await?;

    // Shared CreatorStats and GoLive database.
    let stats_db = sqlx::SqlitePool::connect("sqlite:///app/data/stats.db?mode=rwc").await?;

    commands::creator_stats::database::init_db(&stats_db).await?;
    commands::go_live::database::init_db(&stats_db).await?;

    // Shared HTTP client.
    let http = reqwest::Client::builder()
        .user_agent("CyberPupBot/0.1")
        .build()?;

    // Dedicated Greeter database.
    let greeters_db = sqlx::SqlitePool::connect("sqlite:///app/data/greeters.db?mode=rwc").await?;

    let custom_commands_db =
        sqlx::SqlitePool::connect("sqlite:///app/data/commands.db?mode=rwc").await?;

    commands::custom_commands::database::init_db(&custom_commands_db).await?;

    commands::greeter::database::init_db(&greeters_db).await?;

    // Dedicated ticket database, preserving compatibility with the Python cog.
    let tickets_db = sqlx::SqlitePool::connect("sqlite:///app/data/tickets.db?mode=rwc").await?;
    let counting_db = sqlx::SqlitePool::connect("sqlite:///app/data/counting.db?mode=rwc").await?;
    let moderation_db =
        sqlx::SqlitePool::connect("sqlite:///app/data/moderation.db?mode=rwc").await?;
    let warnings_db = sqlx::SqlitePool::connect("sqlite:///app/data/warnings.db?mode=rwc").await?;
    let message_logger_db =
        sqlx::SqlitePool::connect("sqlite:///app/data/message_logger.db?mode=rwc").await?;
    let reaction_roles_db =
        sqlx::SqlitePool::connect("sqlite:///app/data/reaction_roles.db?mode=rwc").await?;
    let vents_db = sqlx::SqlitePool::connect("sqlite:///app/data/vents.db?mode=rwc").await?;
    commands::tickets::database::init_db(&tickets_db).await?;
    commands::counting::init_db(&counting_db).await?;
    commands::moderation::init_db(&moderation_db).await?;
    commands::warnings::init_db(&warnings_db).await?;
    commands::message_logger::init_db(&message_logger_db).await?;
    commands::reaction_roles::init_db(&reaction_roles_db).await?;
    commands::vents::init_db(&vents_db).await?;

    // Shared Twitch API client.
    let twitch = Arc::new(commands::creator_stats::api::TwitchApi::from_env(
        http.clone(),
    )?);

    commands::sticky::init_db(&db).await?;
    commands::builder::init_db(&db).await?;

    let sticky_channels = Arc::new(RwLock::new(
        commands::sticky::load_sticky_channels(&db).await?,
    ));

    let sticky_locks = Arc::new(RwLock::new(HashMap::new()));

    let sticky_runtime = Arc::new(Mutex::new(StickyRuntime::default()));

    let framework_data = Data {
        db,
        stats_db,
        greeters_db,
        tickets_db,
        counting_db,
        moderation_db,
        warnings_db,
        message_logger_db,
        reaction_roles_db,
        vents_db,
        builder_sessions: Arc::new(Mutex::new(HashMap::new())),
        http,
        twitch,
        sticky_channels,
        sticky_locks,
        sticky_runtime,
        custom_commands_db,
    };

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            event_handler: |framework_ctx, event| {
                Box::pin(handle_event(
                    framework_ctx.serenity_context,
                    event,
                    framework_ctx.user_data,
                ))
            },
            ..Default::default()
        })
        .setup(|ctx, _ready, _framework| {
            Box::pin(async move {
                println!(
                    "🚀 Framework setup complete. Commands sync automatically on GuildCreate."
                );

                // Start CreatorStats background polling.
                commands::creator_stats::polling::start_background(
                    ctx.clone(),
                    framework_data.clone(),
                );
                commands::go_live::polling::start_background(ctx.clone(), framework_data.clone());
                commands::counting::start_background(ctx.clone(), framework_data.clone());

                Ok(framework_data)
            })
        })
        .build();

    let intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::GUILD_VOICE_STATES
        | serenity::GatewayIntents::GUILD_MEMBERS
        | serenity::GatewayIntents::GUILD_MESSAGES
        | serenity::GatewayIntents::MESSAGE_CONTENT;

    let client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .register_songbird()
        .await;

    client?.start().await?;

    Ok(())
}
