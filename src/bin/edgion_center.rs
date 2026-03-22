use edgion::EdgionCenterCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = EdgionCenterCli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
