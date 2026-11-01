//! RegionRoute tables
//!
//! Request selection rules live in RegionRouteConfig. ClusterRegionSetting
//! owns local identity and the destination table. The Gateway reads one immutable
//! setting generation for each request; this module owns no resource table.
