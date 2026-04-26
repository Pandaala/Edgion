use edgion::core::common::startup::init_crypto;
use edgion::EdgionGatewayCli;

fn main() {
    init_crypto();

    let cli = EdgionGatewayCli::parse_args();
    if let Err(err) = cli.run() {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
