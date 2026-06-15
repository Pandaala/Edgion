//! RegionRoute Tables (removed in S3)
//!
//! This module has been emptied as part of the S3 RegionRoute decomposition.
//! The old `ClusterRegionRouteTable` / `ServiceRegionRouteTable` concept is gone.
//! The routing config now lives entirely in `RegionRouteConfig` (EdgionPlugins base);
//! the overlay is `RegionRouteOverride` in EdgionData, resolved by the gateway at
//! request time. No per-resource table is maintained in the controller.
