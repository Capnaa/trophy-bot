mod commands;

use anyhow::Result;
use serenity::all::{ClientBuilder, Command, GuildId};
use serenity::framework::Framework;
use serenity::prelude::*;

use crate::cli::Cli;

#[allow(dead_code)]
pub struct Bot {
    client: Client,
    shard_start: u32,
    shard_end: u32,
    shard_total: u32,
    test_guild_id: Option<GuildId>,
}

impl Bot {
    #[inline]
    pub async fn new(args: &Cli) -> Result<Self> {
        let id = args.bot_id.parse()?;
        let test_guild_id = args.test_guild_id.map(GuildId::from);

        let client = ClientBuilder::from(args)
            .application_id(id)
            .framework(Self::framework(test_guild_id))
            .await?;

        log::debug!("Bot {} initialized", id);
        Ok(Self {
            client,
            shard_start: args.shard_start,
            shard_end: args.shard_end,
            shard_total: args.shard_total,
            test_guild_id,
        })
    }

    #[inline]
    fn framework(test_guild_id: Option<GuildId>) -> impl Framework {
        poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: vec![
                    commands::bench(),
                ],
                ..Default::default()
            })
            .setup(move |ctx, _ready, framework| {
                Box::pin(async move {
                    let builders = poise::builtins::create_application_commands(&framework.options().commands);

                    if let Some(guild_id) = test_guild_id {
                        //poise::builtins::register_in_guild(ctx, &framework.options().commands, guild_id).await?;
                        guild_id.set_commands(&ctx.http, builders).await?;
                        log::warn!("Sync commands in test guild {}", guild_id);
                    } else {
                        //poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                        Command::set_global_commands(&ctx.http, builders).await?;
                        log::info!("Sync commands globally");
                    }
                    Ok(commands::Data {})
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
        } = self;

        let http = client.http.clone();
        tokio::task::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

            if let Some(guild_id) = test_guild_id {
                for command in http.get_guild_commands(guild_id).await? { // TODO: cambiar a guild usado en la config si esta en debug
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
        shard_manager.shutdown_all().await;
        runner.await?.map_err(|e| e.into())
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
