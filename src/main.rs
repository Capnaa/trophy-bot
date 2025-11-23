mod cli;
mod bot;
mod legacy;
mod migrations;

use cli::Cli;
use bot::Bot;
use anyhow::Result;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args = Cli::parse();

    log::warn!("DB URL: {}", args.database_url);
    if args.command.is_some() {
        return migrations::cli(args).await;
    }

    let legacy = legacy::LegacyData::load().await.unwrap();
    //println!("{}", serde_json::to_string_pretty(&legacy.guild(985439832388042822u64)).unwrap());
    println!("{}", serde_json::to_string_pretty(&legacy.guilds()[0]).unwrap());
    //return Ok(());

    Bot::new(&args)
        .await?
        .run()
        .await
}
