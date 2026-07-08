pub mod buttons;
pub mod commands;
pub mod images;
pub mod panel_updater;
pub mod render;
pub mod resolver;
pub mod reward_apply;
pub mod util;

use anyhow::Result;
use sea_orm::DatabaseConnection;
use serenity::all::{ClientBuilder, Command, GuildId};
use serenity::framework::Framework;
use serenity::prelude::*;

use crate::cli::Cli;
use crate::i18n;

/// Shared state available to every command invocation (`ctx.data()`).
pub struct Data {
    /// Connection pool to the normalized database (SQLite locally,
    /// PostgreSQL in production), opened from the CLI's `database_url`.
    pub db: DatabaseConnection,
    /// F29: handle used by score-changing commands (award/revoke/clear) to
    /// request a debounced refresh of the guild's leaderboard panel.
    pub panel_signal: panel_updater::PanelSignal,
}

/// Unified command error type. Commands bubble errors up with `?`; the
/// framework `on_error` handler logs them and answers the user.
pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;

#[allow(dead_code)]
pub struct Bot {
    client: Client,
    shard_start: u32,
    shard_end: u32,
    shard_total: u32,
    test_guild_id: Option<GuildId>,
    /// Background leaderboard-panel updater (F29-F32), joined on shutdown
    /// per ADR 0009.
    panel_task: tokio::task::JoinHandle<()>,
    /// Flipped to `true` by `run` to stop the panel updater gracefully.
    panel_shutdown: tokio::sync::watch::Sender<bool>,
}

impl Bot {
    #[inline]
    pub async fn new(args: &Cli) -> Result<Self> {
        let id = args.bot_id.parse()?;
        let test_guild_id = args.test_guild_id.map(GuildId::from);

        let db = sea_orm::Database::connect(&args.database_url).await?;
        log::info!("Database connection established");

        let (panel_signal, panel_signals) = panel_updater::signal_channel();
        let (panel_shutdown, panel_shutdown_rx) = tokio::sync::watch::channel(false);

        let client = ClientBuilder::from(args)
            .application_id(id)
            .framework(Self::framework(test_guild_id, db.clone(), panel_signal))
            .await?;

        // Background panel updater (ADR 0009: stopped + joined by `run`).
        let panel_task = tokio::task::spawn(panel_updater::run(
            db,
            panel_updater::CacheAndHttp {
                cache: client.cache.clone(),
                http: client.http.clone(),
            },
            panel_signals,
            panel_shutdown_rx,
        ));

        log::debug!("Bot {} initialized", id);
        Ok(Self {
            client,
            shard_start: args.shard_start,
            shard_end: args.shard_end,
            shard_total: args.shard_total,
            test_guild_id,
            panel_task,
            panel_shutdown,
        })
    }

    #[inline]
    fn framework(
        test_guild_id: Option<GuildId>,
        db: DatabaseConnection,
        panel_signal: panel_updater::PanelSignal,
    ) -> impl Framework {
        poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: commands::all(),
                on_error: |error| Box::pin(handle_framework_error(error)),
                event_handler: |ctx, event, framework, data| {
                    Box::pin(buttons::handle_event(ctx, event, framework, data))
                },
                ..Default::default()
            })
            .setup(move |ctx, _ready, framework| {
                Box::pin(async move {
                    let builders = poise::builtins::create_application_commands(&framework.options().commands);

                    if let Some(guild_id) = test_guild_id {
                        guild_id.set_commands(&ctx.http, builders).await?;
                        log::warn!("Sync commands in test guild {}", guild_id);
                    } else {
                        Command::set_global_commands(&ctx.http, builders).await?;
                        log::info!("Sync commands globally");
                    }
                    Ok(Data { db, panel_signal })
                })
            })
            .build()
    }

    #[inline]
    pub async fn run(self) -> Result<()> {
        let Self {
            mut client,
            shard_start,
            shard_end,
            shard_total,
            test_guild_id,
            panel_task,
            panel_shutdown,
        } = self;

        let http = client.http.clone();
        tokio::task::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

            if let Some(guild_id) = test_guild_id {
                for command in http.get_guild_commands(guild_id).await? {
                    log::warn!("Found guild command: {}", command.name);
                }
            } else {
                for command in http.get_global_commands().await? {
                    log::warn!("Found global command: {}", command.name);
                }
            }

            Ok::<(), anyhow::Error>(())
        });

        log::info!("Starting bot");
        let shard_manager = client.shard_manager.clone();
        let runner = tokio::task::spawn(async move {
            client
                .start_shard_range(shard_start..shard_end, shard_total)
                .await
        });

        shutdown_signal().await?;
        log::info!("Shutdown signal received, stopping shards");
        // ADR 0009: background workers stop on the same signal.
        let _ = panel_shutdown.send(true);
        shard_manager.shutdown_all().await;
        if let Err(join_error) = panel_task.await {
            log::error!("Panel updater task failed to join: {join_error}");
        }
        runner.await?.map_err(|e| e.into())
    }
}

/// Central error handler (rust-parity-plan §2): EVERY error path logs with
/// guild/user/command context and answers the user with a friendly localized
/// error embed. No path panics the process or leaves the interaction hanging.
async fn handle_framework_error(error: poise::FrameworkError<'_, Data, Error>) {
    if let poise::FrameworkError::Setup { error, .. } = &error {
        log::error!("Framework setup failed: {error:?}");
        return;
    }

    let Some(ctx) = error.ctx() else {
        log::error!("Framework error without interaction context: {error}");
        return;
    };

    let command = ctx.command().qualified_name.clone();
    let guild = ctx.guild_id().map(|g| g.get());
    let user = ctx.author().id.get();
    let locale = util::locale(&ctx);

    let description = match &error {
        poise::FrameworkError::Command { error, .. } => {
            log::error!("Command /{command} failed (guild={guild:?}, user={user}): {error:?}");
            i18n::t(&locale, "common-error-generic")
        }
        poise::FrameworkError::CommandPanic { payload, .. } => {
            log::error!("Command /{command} panicked (guild={guild:?}, user={user}): {payload:?}");
            i18n::t(&locale, "common-error-generic")
        }
        poise::FrameworkError::CooldownHit { remaining_cooldown, .. } => {
            let seconds = remaining_cooldown.as_secs().max(1);
            log::debug!("Cooldown hit on /{command} (guild={guild:?}, user={user}): {seconds}s left");
            i18n::t_args(&locale, "common-error-cooldown", &[("seconds", seconds.into())])
        }
        poise::FrameworkError::MissingUserPermissions { missing_permissions, .. } => {
            log::warn!(
                "User {user} lacks permissions {missing_permissions:?} for /{command} (guild={guild:?})"
            );
            i18n::t(&locale, "common-error-missing-user-permissions")
        }
        poise::FrameworkError::MissingBotPermissions { missing_permissions, .. } => {
            log::warn!(
                "Bot lacks permissions {missing_permissions} for /{command} (guild={guild:?}, user={user})"
            );
            i18n::t_args(
                &locale,
                "common-error-missing-bot-permissions",
                &[("permissions", missing_permissions.to_string().into())],
            )
        }
        poise::FrameworkError::GuildOnly { .. } => {
            log::debug!("/{command} used outside a guild by user {user}");
            i18n::t(&locale, "common-error-guild-only")
        }
        poise::FrameworkError::NotAnOwner { .. } => {
            log::warn!("Non-owner {user} tried owner-only /{command} (guild={guild:?})");
            i18n::t(&locale, "common-error-not-owner")
        }
        poise::FrameworkError::ArgumentParse { error, input, .. } => {
            log::warn!(
                "Argument parse error on /{command} (guild={guild:?}, user={user}, input={input:?}): {error}"
            );
            i18n::t(&locale, "common-error-invalid-input")
        }
        other => {
            log::error!("Framework error on /{command} (guild={guild:?}, user={user}): {other}");
            i18n::t(&locale, "common-error-generic")
        }
    };

    if let Err(reply_error) = util::reply_error(ctx, description, true).await {
        log::error!(
            "Failed to deliver error reply for /{command} (guild={guild:?}, user={user}): {reply_error:?}"
        );
    }
}

/// Waits for Ctrl+C (SIGINT) or, on Unix, SIGTERM (sent by `docker stop`).
async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = sigterm.recv() => {},
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
