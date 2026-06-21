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

fn main() {
    install_panic_hook();
    init_crypto();

    let cli = EdgionGatewayCli::parse_args();
    if let Err(err) = cli.run() {
        report_startup_failure(&err);
        std::process::exit(1);
    }
}
