use clap::Parser;
use dotenv::dotenv;
use serenity::all::{GatewayIntents, Http};
use serenity::Client;
use serenity::client::ClientBuilder;

#[derive(Parser)] // Does not implement Debug to avoid leaking sensitive info.
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(long, default_value_t = false, env = "DEBUG")]
    pub debug: bool,
    #[arg(long, env = "TEST_GUILD_ID")]
    pub test_guild_id: Option<u64>,
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,
    #[arg(long, env = "DISCORD_BOT_ID")]
    pub bot_id: String,
    #[arg(long, env = "DISCORD_TOKEN")]
    token: String, // Avoid to make it public for security reasons.
}

impl Cli {
    pub fn parse() -> Self {
        env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .init();

        log::info!("Starting {} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        if let Err(e) = dotenv() {
            log::error!("Can not load .env file: {}", e);
        }

        let args: Self = Parser::parse();
        if args.debug {
            log::debug!("Options for bot ID {} parsed", args.bot_id);
        } else {
            log::set_max_level(log::LevelFilter::Warn);
        }

        args
    }
}

// This keeps the token private and avoids accidental logging, at least in our code.

impl From<&Cli> for Http {
    #[inline]
    fn from(args: &Cli) -> Self {
        Http::new(args.token.as_str())
    }
}

impl From<&Cli> for ClientBuilder {
    #[inline]
    fn from(args: &Cli) -> Self {
        Client::builder(args.token.as_str(), GatewayIntents::non_privileged())
    }
}
