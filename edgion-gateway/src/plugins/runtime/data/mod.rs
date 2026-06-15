//! EdgionData gateway store and ConfHandler
//!
//! Provides a global ArcSwap-backed store with pre-indexed typed maps for
//! zero-clone lookups at request time (e.g., RegionRoute resolving the
//! EdgionData chain).

mod conf_handler;

pub use conf_handler::{
    count_edgion_data, create_edgion_data_handler, edgion_data_keys, get_global_edgion_data_store, EdgionDataStore,
};
