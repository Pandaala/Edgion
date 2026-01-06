use edgion::EdgionGatewayCli;

fn main() {
    // Install rustls crypto provider (required for rustls 0.22+)
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let cli = EdgionGatewayCli::parse_args();
    if let Err(err) = cli.run() {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
