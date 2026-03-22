use edgion::EdgionCenterCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("info".parse().unwrap())
        )
        .init();

    let cli = EdgionCenterCli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
