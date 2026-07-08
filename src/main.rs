mod cli;
mod bot;
mod domain;
mod entities;
mod i18n;
mod import;
mod legacy;
mod migrations;
mod smoke;

use cli::Cli;
use bot::Bot;
use anyhow::Result;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args = Cli::parse();

    log::debug!("DB URL: {}", args.database_url);
    if args.command == Some(migrations::MigrateSubcommands::Smoke) {
        // The smoke flow manages its own disposable database; it never uses
        // `--database-url`.
        return smoke::run(&args).await;
    }
    if args.command.is_some() {
        return migrations::cli(args).await;
    }

    Bot::new(&args)
        .await?
        .run()
        .await
}
