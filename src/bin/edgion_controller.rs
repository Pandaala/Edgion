use edgion::EdgionControllerCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // Install rustls crypto provider (required for rustls 0.22+)
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = EdgionControllerCli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
