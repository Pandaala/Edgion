use edgion::EdgionGatewayCli;
use edgion_common::startup::{init_crypto, install_panic_hook};

fn main() {
    install_panic_hook();
    init_crypto();

    let cli = EdgionGatewayCli::parse_args();
    if let Err(err) = cli.run() {
        eprintln!("Error: {:#}", err);
        std::process::exit(1);
    }
}
