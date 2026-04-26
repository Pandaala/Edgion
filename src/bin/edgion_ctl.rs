use clap::Parser;
use edgion::core::common::startup::init_crypto;
use edgion::core::ctl::Cli;

#[tokio::main]
async fn main() {
    init_crypto();

    let cli = Cli::parse();

    if let Err(e) = cli.run().await {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
