pub mod cert_watch;
mod config_client;
mod grpc_client;
mod rotation;

use std::sync::{Arc, LazyLock};
use tokio_util::sync::CancellationToken;

pub use config_client::ConfigClient;
pub use grpc_client::ConfigSyncClient;

/// Process-global "reload generation" token for the conf_sync client TLS.
///
/// The per-kind watch loops (`cache_client::event_dispatch`) have no handle to
/// the owning [`ConfigSyncClient`], so they read the current generation token
/// through [`current_reload_generation`]. When the channel publisher swaps the
/// channel it installs a fresh token here, then cancels the previous one; every
/// running watch loop selecting on the old token breaks out and reconnects on
/// the new channel. Initialized with a never-cancelled token so the very first
/// watch loops have something to select on before any reload happens.
static RELOAD_GENERATION: LazyLock<arc_swap::ArcSwap<CancellationToken>> =
    LazyLock::new(|| arc_swap::ArcSwap::from_pointee(CancellationToken::new()));

/// Load the current reload-generation token. Watch loops capture this once per
/// reconnect cycle and select on `token.cancelled()` to break out promptly when
/// the client TLS channel is hot-reloaded.
pub fn current_reload_generation() -> Arc<CancellationToken> {
    RELOAD_GENERATION.load_full()
}

/// Publish a new reload-generation token (called by `ConfigSyncClient::new` at
/// startup and by the channel publisher on each replacement).
pub(crate) fn set_global_reload_generation(token: Arc<CancellationToken>) {
    RELOAD_GENERATION.store(token);
}
