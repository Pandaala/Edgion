// Memory allocator selection lives with the binary.
#[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
use tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[cfg(feature = "allocator-mimalloc")]
use mimalloc::MiMalloc;

#[cfg(feature = "allocator-mimalloc")]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use edgion_common::startup::{init_crypto, install_panic_hook, report_startup_failure};
use edgion_gateway::EdgionGatewayCli;
use std::io::Write;

fn main() {
    install_panic_hook();

    let cli = EdgionGatewayCli::parse_args();
    if cli.print_build_capabilities {
        let _ = std::io::stdout().write_all(edgion_gateway::build_capabilities().as_bytes());
        return;
    }

    init_crypto();
    if let Err(err) = cli.run() {
        report_startup_failure(&err);
        std::process::exit(1);
    }
}
