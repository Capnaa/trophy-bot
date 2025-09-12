use anyhow::Result;
use serenity::all::{ClientBuilder, CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage, Ready};
use serenity::async_trait;
use serenity::http::Http;
use serenity::prelude::*;

use crate::cli::Cli;

pub struct Bot {
    http: Http,
    client: Client,
}

impl Bot {
    #[inline]
    pub async fn new(args: &Cli) -> Result<Self> {
        let intents = GatewayIntents::GUILDS;

        let id = args.bot_id.parse().expect("Invalid bot ID");
        let http = Http::from(args);
        http.set_application_id(id);

        let client = ClientBuilder::from(args)
            .intents(intents)
            .event_handler(Handler)
            .await?;

        let bot = Self { http, client };
        bot.register_commands().await?;

        Ok(bot)
    }

    async fn register_commands(&self) -> Result<()> {
        let command = CreateCommand::new("ping")
            .description("Test command - responds with Trophy Bot 2.0");

        self.http.create_global_command(&command).await?;
        Ok(())
    }

    pub async fn run(&mut self) -> Result<()> {
        self.client.start().await?;
        Ok(())
    }
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        log::info!("🎯 Bot ready! Logged in as: {}", ready.user.name);
        
        // Log shard and session information
        let shard_id = ctx.shard_id;
        log::info!("💗 Shard ID: {} | Session ID: {}", shard_id, ready.session_id);
        log::info!("💗 Resume Gateway URL: {}", ready.resume_gateway_url);
        
        // Log heartbeat management info
        log::info!("💗 Heartbeat: Managed automatically by Serenity");
        log::info!("💗 Interval: Determined by Discord's Hello payload (OP 10)");
    }

    async fn interaction_create(&self, ctx: Context, interaction: serenity::all::Interaction) {
        if let serenity::all::Interaction::Command(command) = interaction {
            log::info!("📝 Command: /{}", command.data.name);
            
            let response = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("🏆 Trophy Bot 2.0 - Rust Edition is working! 🦀")
            );
            
            if let Err(e) = command.create_response(&ctx.http, response).await {
                log::error!("Failed to respond: {}", e);
            }
        }
    }
}
