//! Hostname Intersection attachedRoutes Status Test Suite
//!
//! Tests that Gateway status.listeners[].attachedRoutes only counts
//! routes whose hostnames intersect with the listener's hostname.
//!
//! Required config files (in examples/test/conf/Status/HostnameIntersection/):
//! - 01_Gateway.yaml
//! - HTTPRoute_specific-matches-specific.yaml
//! - HTTPRoute_specific-matches-wildcard.yaml
//! - HTTPRoute_wildcard-matches-specific.yaml
//! - HTTPRoute_wildcard-matches-wildcard.yaml
//! - HTTPRoute_no-intersecting-hosts.yaml

mod status_test;

pub use status_test::HostnameIntersectionStatusTestSuite;
