pub mod cache_client;
pub mod conf_client;

use std::sync::{Arc, OnceLock};

pub use cache_client::{ClientCache, DynClientCache};
pub use conf_client::{ConfigClient, ConfigSyncClient, ListDataSimple};

static GLOBAL_CONFIG_CLIENT: OnceLock<Arc<ConfigClient>> = OnceLock::new();

pub fn init_global_config_client(config_client: Arc<ConfigClient>) -> Result<(), String> {
    GLOBAL_CONFIG_CLIENT
        .set(config_client)
        .map_err(|_| "Global ConfigClient already initialized".to_string())
}

pub fn get_global_config_client() -> Option<Arc<ConfigClient>> {
    GLOBAL_CONFIG_CLIENT.get().cloned()
}
