pub mod buttons;
pub mod commands;
pub mod images;
pub mod medals_panel;
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
    /// Handle used by trophy-editing commands (create/edit/delete) to
    /// request a debounced refresh of a category's medals catalog panel.
    pub medals_panel_signal: medals_panel::PanelSignal,
}

/// Unified command error type. Commands bubble errors up with `?`; the
/// framework `on_error` handler logs them and answers the user.
pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;

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
    /// Background medals-catalog-panel updater, joined on shutdown per
    /// ADR 0009 (same lifecycle as `panel_task`).
    medals_panel_task: tokio::task::JoinHandle<()>,
    /// Flipped to `true` by `run` to stop the medals panel updater gracefully.
    medals_panel_shutdown: tokio::sync::watch::Sender<bool>,
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
        let (medals_panel_signal, medals_panel_signals) = medals_panel::signal_channel();
        let (medals_panel_shutdown, medals_panel_shutdown_rx) = tokio::sync::watch::channel(false);

        let client = ClientBuilder::from(args)
            .application_id(id)
            .framework(Self::framework(
                test_guild_id,
                db.clone(),
                panel_signal,
                medals_panel_signal,
            ))
            .await?;

        // Background panel updaters (ADR 0009: stopped + joined by `run`).
        let cache_http =
            panel_updater::CacheAndHttp { cache: client.cache.clone(), http: client.http.clone() };
        let panel_task = tokio::task::spawn(panel_updater::run(
            db.clone(),
            cache_http.clone(),
            panel_signals,
            panel_shutdown_rx,
        ));
        let medals_panel_task = tokio::task::spawn(medals_panel::run(
            db,
            cache_http,
            medals_panel_signals,
            medals_panel_shutdown_rx,
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
            medals_panel_task,
            medals_panel_shutdown,
        })
    }

    #[inline]
    fn framework(
        test_guild_id: Option<GuildId>,
        db: DatabaseConnection,
        panel_signal: panel_updater::PanelSignal,
        medals_panel_signal: medals_panel::PanelSignal,
    ) -> impl Framework {
        poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: commands::all(),
                on_error: |error| Box::pin(handle_framework_error(error)),
                // rust-parity-plan §2: count SUCCESSFUL executions only.
                // Poise invokes `post_command` only after the command action
                // returned `Ok`, so failed runs never bump the counters
                // (they are logged by `handle_framework_error` instead).
                post_command: |ctx| Box::pin(record_command_run(ctx)),
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
                        log::info!("Sync commands in test guild {}", guild_id);
                    } else {
                        Command::set_global_commands(&ctx.http, builders).await?;
                        log::info!("Sync commands globally");
                    }
                    Ok(Data { db, panel_signal, medals_panel_signal })
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
            medals_panel_task,
            medals_panel_shutdown,
        } = self;

        // One-shot diagnostic: list what Discord actually registered. The
        // JoinHandle is deliberately detached (harmless if dropped during
        // shutdown), so every outcome — success AND failure — must be logged
        // right here; a `?` would vanish with the discarded handle.
        let http = client.http.clone();
        tokio::task::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

            let listing = if let Some(guild_id) = test_guild_id {
                http.get_guild_commands(guild_id).await
            } else {
                http.get_global_commands().await
            };
            match listing {
                Ok(registered) => {
                    let names: Vec<String> =
                        registered.into_iter().map(|command| command.name).collect();
                    log::info!(
                        "Commands registered on Discord: {}",
                        format_command_listing(&names)
                    );
                }
                Err(error) => {
                    log::error!("Failed to list registered commands after startup: {error:?}");
                }
            }
        });

        log::info!("Starting bot");
        let shard_manager = client.shard_manager.clone();
        let mut runner = tokio::task::spawn(async move {
            client
                .start_shard_range(shard_start..shard_end, shard_total)
                .await
        });

        let early_exit = until_shutdown_or_runner_exit(shutdown_signal(), &mut runner).await;
        match &early_exit {
            None => log::info!("Shutdown signal received, stopping shards"),
            Some(_) => {
                log::error!("Shard runner exited before any shutdown signal; shutting down")
            }
        }
        // ADR 0009: background workers stop on the same signal (or on the
        // runner dying early — otherwise the panel updater would keep
        // sweeping a dead bot forever).
        let _ = panel_shutdown.send(true);
        let _ = medals_panel_shutdown.send(true);
        shard_manager.shutdown_all().await;
        if let Err(join_error) = panel_task.await {
            log::error!("Panel updater task failed to join: {join_error}");
        }
        if let Err(join_error) = medals_panel_task.await {
            log::error!("Medals panel updater task failed to join: {join_error}");
        }
        let runner_result = match early_exit {
            Some(result) => result,
            None => runner.await,
        };
        runner_result?.map_err(|e| e.into())
    }
}

/// Waits for a shutdown signal, but also notices the shard runner finishing
/// on its own (invalid token, gateway boot failure) so the process exits and
/// a supervisor can restart it instead of hanging in signal-wait forever with
/// the panel updater still sweeping. Returns `Some(join_result)` when the
/// runner exited first, `None` on a shutdown request. A failed signal
/// listener is treated as a shutdown request (logged) so cleanup still runs.
async fn until_shutdown_or_runner_exit<T>(
    shutdown: impl std::future::Future<Output = Result<()>>,
    runner: &mut tokio::task::JoinHandle<T>,
) -> Option<std::result::Result<T, tokio::task::JoinError>> {
    tokio::select! {
        signal = shutdown => {
            if let Err(error) = signal {
                log::error!("Shutdown signal listener failed, stopping anyway: {error:?}");
            }
            None
        }
        result = runner => Some(result),
    }
}

/// Renders the startup command-listing diagnostic: `N (a, b, c)` or `none`.
fn format_command_listing(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        format!("{} ({})", names.len(), names.join(", "))
    }
}

/// `post_command` hook (rust-parity-plan §2): bump the success-only run
/// counters for every command, keyed like the legacy dispatcher.
async fn record_command_run(ctx: Context<'_>) {
    let command = root_command_name(ctx.parent_commands(), ctx.command());
    record_run_counters(&ctx.data().db, command).await;
}

/// Legacy parity: run counters are keyed by the TOP-LEVEL command name
/// (`data.commands.${interaction.commandName}`, events/command.js), so
/// subcommand invocations like `/rewards add` count under `rewards`.
fn root_command_name<'a>(
    parents: &'a [&'a poise::Command<Data, Error>],
    invoked: &'a poise::Command<Data, Error>,
) -> &'a str {
    parents.first().map_or(invoked.name.as_str(), |root| root.name.as_str())
}

/// Bumps the `total` + per-command counters in `bot_stats`. Failures are
/// logged and swallowed: a counter error must never surface to a user whose
/// command already succeeded.
async fn record_run_counters(db: &DatabaseConnection, command: &str) {
    if let Err(error) = commands::stats::record_successful_run(db, command).await {
        log::error!("Failed to record /{command} run in bot_stats: {error:?}");
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

    // `reply_error_ephemeral`, not the plain `reply_error`: commands that
    // deferred PUBLICLY before failing (/leaderboard, /show, the /create and
    // /edit image paths) would otherwise surface the error publicly —
    // Discord locks a deferred response's visibility at defer time (§2: all
    // error replies are ephemeral).
    if let Err(reply_error) = util::reply_error_ephemeral(ctx, description).await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::bot_stats;
    use crate::migrations::Migrator;
    use sea_orm::{ColumnTrait, ConnectOptions, Database, EntityTrait, QueryFilter};
    use sea_orm_migration::MigratorTrait;

    // -- handle_framework_error i18n catalog coverage ------------------------

    /// Every key `handle_framework_error` (and its `reply_error` helper) can
    /// answer with must exist in the en-US catalog — `i18n::t` falls back to
    /// the raw key id, which would otherwise ship to users unnoticed.
    #[test]
    fn framework_error_messages_exist_in_catalog() {
        let locale = i18n::resolve(None);
        for key in [
            "common-error-title",
            "common-error-generic",
            "common-error-missing-user-permissions",
            "common-error-guild-only",
            "common-error-not-owner",
            "common-error-invalid-input",
        ] {
            assert_ne!(i18n::t(&locale, key), key, "missing catalog entry for {key}");
        }
    }

    /// The two parameterized error messages must interpolate their arguments.
    #[test]
    fn framework_error_messages_interpolate_arguments() {
        let locale = i18n::resolve(None);

        for seconds in [1u64, 7] {
            let cooldown = i18n::t_args(
                &locale,
                "common-error-cooldown",
                &[("seconds", seconds.into())],
            );
            assert_ne!(cooldown, "common-error-cooldown");
            assert!(
                cooldown.contains(&seconds.to_string()),
                "cooldown message missing {seconds}: {cooldown}"
            );
        }

        let bot_permissions = i18n::t_args(
            &locale,
            "common-error-missing-bot-permissions",
            &[("permissions", "Manage Roles".into())],
        );
        assert_ne!(bot_permissions, "common-error-missing-bot-permissions");
        assert!(
            bot_permissions.contains("Manage Roles"),
            "got: {bot_permissions}"
        );
    }

    // -- success-only run counters (post_command hook) -----------------------

    async fn fresh_db() -> DatabaseConnection {
        // Single connection: each pooled connection to `sqlite::memory:`
        // would otherwise get its own private database.
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let db = Database::connect(options).await.expect("connect to in-memory sqlite");
        Migrator::fresh(&db).await.expect("apply migrations");
        db
    }

    async fn counter(db: &DatabaseConnection, name: &str) -> Option<i64> {
        bot_stats::Entity::find()
            .filter(bot_stats::Column::Name.eq(name))
            .one(db)
            .await
            .expect("read counter")
            .map(|row| row.total)
    }

    #[tokio::test]
    async fn record_run_counters_bumps_total_and_per_command() {
        let db = fresh_db().await;

        record_run_counters(&db, "award").await;
        record_run_counters(&db, "award").await;
        record_run_counters(&db, "stats").await;

        assert_eq!(counter(&db, "total").await, Some(3));
        assert_eq!(counter(&db, "award").await, Some(2));
        assert_eq!(counter(&db, "stats").await, Some(1));
    }

    /// The hook swallows database errors (here: no `bot_stats` table at all)
    /// — a counter failure must never take down an already-answered command.
    #[tokio::test]
    async fn record_run_counters_swallows_database_errors() {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(1).sqlx_logging(false);
        let db = Database::connect(options).await.expect("connect to in-memory sqlite");

        record_run_counters(&db, "award").await; // must not panic or error
    }

    /// Legacy parity: `/rewards add` counts under `rewards`, exactly like the
    /// old dispatcher's `data.commands.${interaction.commandName}`.
    #[test]
    fn run_counters_are_keyed_by_top_level_command_name() {
        let all = commands::all();
        let rewards = all
            .iter()
            .find(|c| c.name == "rewards")
            .expect("rewards registered");
        let add = rewards
            .subcommands
            .iter()
            .find(|c| c.name == "add")
            .expect("rewards add registered");
        assert_eq!(root_command_name(&[rewards], add), "rewards");

        let ping = all.iter().find(|c| c.name == "ping").expect("ping registered");
        assert_eq!(root_command_name(&[], ping), "ping");
    }

    // -- startup command-listing diagnostic ----------------------------------

    #[test]
    fn command_listing_formats_names_and_empty_case() {
        assert_eq!(format_command_listing(&[]), "none");
        let names = ["ping".to_owned(), "award".to_owned(), "stats".to_owned()];
        assert_eq!(format_command_listing(&names), "3 (ping, award, stats)");
    }

    // -- shutdown vs. early shard-runner exit ---------------------------------

    #[tokio::test]
    async fn shutdown_signal_wins_while_runner_is_alive() {
        let mut runner = tokio::task::spawn(async {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            7u8
        });
        let exit = until_shutdown_or_runner_exit(async { Ok(()) }, &mut runner).await;
        assert!(exit.is_none(), "a shutdown signal must not report an early exit");
        runner.abort();
    }

    #[tokio::test]
    async fn early_runner_exit_is_detected_without_a_signal() {
        let mut runner = tokio::task::spawn(async { 7u8 });
        let exit =
            until_shutdown_or_runner_exit(std::future::pending::<Result<()>>(), &mut runner)
                .await;
        let join_result = exit.expect("runner death must end the wait");
        assert_eq!(join_result.expect("runner task joined"), 7);
    }

    /// A broken signal listener degrades to an immediate shutdown request so
    /// the cleanup path (panel updater, shards) still runs.
    #[tokio::test]
    async fn failed_signal_listener_is_treated_as_shutdown() {
        let mut runner = tokio::task::spawn(async {
            tokio::time::sleep(tokio::time::Duration::from_secs(3600)).await;
            7u8
        });
        let exit = until_shutdown_or_runner_exit(
            async { Err(anyhow::anyhow!("no signal handler")) },
            &mut runner,
        )
        .await;
        assert!(exit.is_none());
        runner.abort();
    }
}
