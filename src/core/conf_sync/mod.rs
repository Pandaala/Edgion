mod cache_client;
mod cache_server;
mod proto;
pub mod traits;
pub mod types;

pub mod conf_client;
pub mod conf_server;
pub mod conf_server_new;
pub use crate::types::{GatewayBaseConf, ResourceMeta};
pub use cache_client::ClientCache;
pub use cache_server::ServerCache;
pub use conf_client::{ConfigClient, ConfigSyncClient};
// Re-export from new conf_server module
pub use conf_server::{ConfigServer, ConfigSyncServer, ServerCacheObj, WatchResponseSimple};
pub use traits::{CacheEventDispatch, ConfHandler};

use std::sync::{Arc, OnceLock};

/// Global ConfigClient instance
static GLOBAL_CONFIG_CLIENT: OnceLock<Arc<ConfigClient>> = OnceLock::new();

/// Initialize the global ConfigClient
/// This should be called once during application startup
/// Returns error if already initialized
pub fn init_global_config_client(config_client: Arc<ConfigClient>) -> Result<(), String> {
    GLOBAL_CONFIG_CLIENT
        .set(config_client)
        .map_err(|_| "Global ConfigClient already initialized".to_string())
}

/// Get reference to the global ConfigClient
/// Returns None if not yet initialized
pub fn get_global_config_client() -> Option<Arc<ConfigClient>> {
    GLOBAL_CONFIG_CLIENT.get().cloned()
}
