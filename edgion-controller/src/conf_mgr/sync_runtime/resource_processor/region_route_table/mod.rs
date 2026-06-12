//! RegionRoute Tables
//!
//! Pre-built indexes for RegionRoute data maintained by the Controller.
//! Updated incrementally by PluginMetaData and EdgionPlugins handlers on change/delete.
//! API handlers read from these tables instead of scanning all resources.
//!
//! ## Tables
//!
//! - `RegionRouteClusterTable`: serviceGroup → ClusterRegionRouteEntry
//! - `RegionRouteServiceTable`: pm_key (ns/name) → ServiceRegionRouteEntry

mod cluster_table;
mod service_table;

pub use cluster_table::{ClusterRegionRouteEntry, RegionRouteClusterTable};
pub use service_table::{RefPluginEntry, RegionRouteServiceTable, ServiceRegionRouteEntry};
