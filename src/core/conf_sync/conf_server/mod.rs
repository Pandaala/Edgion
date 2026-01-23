//! New conf_server module with ServerCacheObj trait abstraction
//!
//! This module replaces the old conf_server_old module with a more flexible design:
//! - ServerCacheObj trait abstracts ServerCache<T> interface
//! - ServerCacheFactory manages all caches via HashMap<String, Arc<dyn ServerCacheObj>>
//! - ConfigServer uses the factory for simplified list/watch operations

mod conf_change_apply;
mod config_server;
mod factory;
mod grpc_server;
mod traits;

pub use config_server::{ConfigServer, EventDataSimple, ListDataSimple, ResourceItem};
pub use factory::{kind_names, ServerCacheFactory};
pub use grpc_server::ConfigSyncServer;
pub use traits::{ServerCacheObj, WatchResponseSimple};

// Backward compatibility re-exports from resource_processor::secret_utils
// These types have been moved to sync_runtime/resource_processor/secret_utils/
pub use crate::core::conf_mgr::conf_center::sync_runtime::resource_processor::{
    get_secret, get_secret_by_name, update_secrets, RefManagerStats, ResourceRef, SecretRefManager, SecretStore,
};
