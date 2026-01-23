// Old module kept for reference only
// New implementation is in conf_server module

// Note: conf_change_apply is removed because it defines duplicate impl methods
// for ConfigServer which now lives in conf_server module

mod config_server;
mod grpc_server;
mod secret_ref;
pub mod secret_store;

// Re-export from the new conf_server module for backward compatibility
pub use crate::core::conf_sync::conf_server::{
    ConfigServer, ConfigSyncServer, EventDataSimple, ListDataSimple, RefManagerStats, ResourceItem,
    ResourceRef, SecretRefManager,
};
pub use secret_store::{get_secret_by_name, replace_all_secrets, update_secrets, SecretStore};

// NsNameKey is a simple type alias, keep it here
pub type NsNameKey = String;
