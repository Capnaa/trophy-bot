use clap::Parser;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
}

#[tokio::main]
async fn main() {
    log::info!("Starting {} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    let args = Cli::parse();
}
