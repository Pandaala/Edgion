mod cache_server;
mod cache_client;
mod proto;
pub mod traits;
pub mod types;

pub mod conf_client;
pub mod conf_server;

pub use cache_client::ClientCache;
pub use cache_server::ServerCache;
pub use conf_client::{ConfigClient, ConfigSyncClient};
pub use conf_server::{ConfigServer, ConfigSyncServer};
pub use traits::{CacheEventDispatch, ConfHandler, ConfigServerEventDispatcher};
pub use crate::types::{GatewayBaseConf, ResourceMeta};

use std::sync::OnceLock;

/// Global ConfigSyncClient instance
static GLOBAL_SYNC_CLIENT: OnceLock<ConfigSyncClient> = OnceLock::new();

/// Initialize the global ConfigSyncClient
/// This should be called once during application startup
/// Returns error if already initialized
pub fn init_global_sync_client(sync_client: ConfigSyncClient) -> Result<(), String> {
    GLOBAL_SYNC_CLIENT
        .set(sync_client)
        .map_err(|_| "Global ConfigSyncClient already initialized".to_string())
}

/// Get reference to the global ConfigSyncClient
/// Returns None if not yet initialized
pub fn get_global_sync_client() -> Option<&'static ConfigSyncClient> {
    GLOBAL_SYNC_CLIENT.get()
}

/// Get reference to the global ConfigClient through the sync client
/// Returns None if sync client not yet initialized
pub fn get_global_config_client() -> Option<std::sync::Arc<ConfigClient>> {
    GLOBAL_SYNC_CLIENT.get().map(|sc| sc.get_config_client())
}

#[cfg(test)]
mod tests;
