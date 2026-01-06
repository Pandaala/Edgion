use clap::Parser;
use edgion::core::cli::edgion_ctl::Cli;

#[tokio::main]
async fn main() {
    // Install rustls crypto provider (required for rustls 0.22+)
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Parse CLI arguments
    let cli = Cli::parse();

    // Run the command
    if let Err(e) = cli.run().await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
