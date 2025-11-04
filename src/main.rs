mod cli;
mod bot;
mod legacy;

use cli::Cli;
use bot::Bot;
use anyhow::Result;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let args = Cli::parse();
    Bot::new(&args)
        .await?
        .run()
        .await
}
