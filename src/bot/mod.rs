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
}

impl Bot {
    #[inline]
    pub async fn new(args: &Cli) -> Result<Self> {
        let id = args.bot_id.parse()?;
        let client = ClientBuilder::from(args)
            .application_id(id)
            .framework(Self::framework(args.test_guild_id))
            .await?;

        log::debug!("Bot {} initialized", id);
        Ok(Self {
            client,
            shard_start: args.shard_start,
            shard_end: args.shard_end,
            shard_total: args.shard_total,
        })
    }

    #[inline]
    fn framework(test_guild_id: Option<u64>) -> impl Framework {
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
                        GuildId::from(guild_id)
                            .set_commands(&ctx.http, builders)
                            .await?;
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
        } = self;

        let http = client.http.clone();
        tokio::task::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

            for command in http.get_global_commands().await? {
                log::warn!("Found global command: {}", command.name);
            }

            for command in http.get_guild_commands(985439832388042822.into()).await? { // TODO: cambiar a guild usado en la config si esta en debug
                log::warn!("Found guild command: {}", command.name);
            }

            Ok::<(), anyhow::Error>(())
        });

        log::info!("Starting bot");
        client
            .start_shard_range(shard_start..shard_end, shard_total)
            .await
            .map_err(|e| e.into())
    }
}
