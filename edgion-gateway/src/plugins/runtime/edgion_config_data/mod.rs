//! EdgionConfigData gateway store and ConfHandler
//!
//! Provides a global ArcSwap-backed store with pre-indexed typed maps for
//! zero-clone lookups at request time (e.g., RegionRoute resolving the
//! EdgionConfigData chain).

mod conf_handler;

pub use conf_handler::{
    count_edgion_config_data, create_edgion_config_data_handler, edgion_config_data_keys,
    get_global_edgion_config_data_store, resolve_ip_list_matchers, resolve_ip_list_ref_keys, EdgionConfigDataStore,
};
