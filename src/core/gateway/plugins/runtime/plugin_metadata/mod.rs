//! PluginMetaData gateway store and ConfHandler
//!
//! Provides a global ArcSwap-backed store with pre-indexed typed maps for
//! zero-clone lookups at request time (e.g., RegionRoute resolving the
//! PluginMetaData chain).

mod conf_handler;

pub use conf_handler::{create_plugin_metadata_handler, get_global_plugin_metadata_store, PluginMetaDataGatewayStore};
