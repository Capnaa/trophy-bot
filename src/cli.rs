use clap::Parser;

#[derive(Parser)] // Does not implement Debug to avoid leaking sensitive info.
#[command(version, about, long_about = None)]
pub struct Cli {
    #[arg(long, default_value_t = false, env = "DEBUG")]
    pub debug: bool,
    #[arg(long, env = "DISCORD_BOT_ID")]
    pub bot_id: String,
    #[arg(long, env = "DISCORD_TOKEN")]
    token: String, // Avoid to make it public for security reasons.
}

impl Cli {
    pub fn parse() -> Self {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .init();

        log::info!("Starting {} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        if let Err(e) = dotenv::load() {
            log::error!("Can not load .env file: {}", e);
        }

        let args: Self = Parser::parse();
        if args.debug {
            log::set_max_level(log::LevelFilter::Debug);
            log::debug!("Options for bot ID {} loaded", args.bot_id);
        } else {
            log::set_max_level(log::LevelFilter::Info);
        }

        args
    }
}
