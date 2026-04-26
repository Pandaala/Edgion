use edgion::core::common::startup::init_crypto;
use edgion::EdgionCenterCli;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    init_crypto();

    let cli = EdgionCenterCli::parse_args();
    if let Err(err) = cli.run().await {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
