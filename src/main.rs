mod cli;
mod bot;
mod legacy;

use cli::Cli;
use bot::Bot;
use anyhow::Result;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let legacy = legacy::LegacyData::load().await;
    //println!("{}", serde_json::to_string_pretty(&legacy.guild(985439832388042822u64)).unwrap());
    println!("{}", serde_json::to_string_pretty(&legacy.guilds()[0]).unwrap());
    //return Ok(());

    Bot::new(&args)
        .await?
        .run()
        .await
}
