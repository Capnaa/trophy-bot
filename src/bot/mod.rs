mod commands;

use anyhow::Result;
use serenity::all::ClientBuilder;
use serenity::framework::Framework;
use serenity::http::Http;
use serenity::prelude::*;

use crate::cli::Cli;

#[allow(dead_code)]
pub struct Bot {
    http: Http,
    client: Client,
}

impl Bot {
    #[inline]
    pub async fn new(args: &Cli) -> Result<Self> {
        let id = args.bot_id.parse().expect("Invalid bot ID");
        let http = Http::from(args);
        http.set_application_id(id);

        let client = ClientBuilder::from(args)
            .framework(Self::framework(args.test_guild_id))
            .await?;

        for command in http.get_global_commands().await? {
            println!("Registered global command: {}", command.name);
        }

        for command in http.get_guild_commands(1393760778972041258.into()).await? {
            println!("Registered guild command: {}", command.name);
        }

        log::debug!("Bot {} initialized", id);
        Ok(Self { http, client })
    }

    fn framework(test_guild_id: Option<u64>) -> impl Framework {
        poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: vec![commands::age()],
                ..Default::default()
            })
            .setup(move |ctx, _ready, framework| {
                Box::pin(async move {
                    if let Some(guild_id) = test_guild_id {
                        poise::builtins::register_in_guild(ctx, &framework.options().commands, guild_id.into()).await?;
                        log::warn!("Sync commands in test guild {}", guild_id);
                    } else {
                        poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                        log::info!("Sync commands globally");
                    }
                    Ok(commands::Data {})
                })
            })
            .build()
    }

    pub async fn run(&mut self) -> Result<()> {
        log::info!("Starting bot");
        self.client
            .start()
            .await?;

        Ok(())
    }
}
