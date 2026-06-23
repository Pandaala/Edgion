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

use clap::Parser;
use edgion_cli::Cli;
use edgion_common::startup::init_crypto;

#[tokio::main]
async fn main() {
    init_crypto();

    let cli = Cli::parse();

    if let Err(e) = cli.run().await {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
