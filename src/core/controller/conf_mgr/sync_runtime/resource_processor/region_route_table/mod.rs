//! RegionRoute Tables
//!
//! Pre-built indexes for RegionRoute data maintained by the Controller.
//! Updated incrementally by PluginMetaData and EdgionPlugins handlers on change/delete.
//! API handlers read from these tables instead of scanning all resources.
//!
//! ## Tables
//!
//! - `RegionRouteGlobalTable`: serviceGroup → GlobalRegionRouteEntry
//! - `RegionRouteServiceTable`: pm_key (ns/name) → ServiceRegionRouteEntry

mod global_table;
mod service_table;

pub use global_table::{GlobalRegionRouteEntry, RegionRouteGlobalTable};
pub use service_table::{RefPluginEntry, RegionRouteServiceTable, ServiceRegionRouteEntry};
