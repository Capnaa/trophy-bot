mod cli;
use cli::Cli;

#[tokio::main]
async fn main() {
    let args = Cli::parse(); // Keep this always as the first line of main.

    println!("{} {}", args.bot_id, args.debug);
}
