use edgion::core::common::startup::{init_crypto, install_panic_hook};
use edgion::EdgionCenterCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    install_panic_hook();
    init_crypto();

    let cli = EdgionCenterCli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
