//! ReferenceGrant and Status System Test Suite
//!
//! Tests cross-namespace reference validation and status updates.
//!
//! Required config files (in examples/test/conf/ref-grant-status/):
//! - Service_backend_cross-ns-svc.yaml
//! - EndpointSlice_backend_cross-ns-svc.yaml
//! - HTTPRoute_app_cross-ns-route.yaml
//! - HTTPRoute_app_cross-ns-denied.yaml
//! - HTTPRoute_app_multi-parent.yaml
//! - ReferenceGrant_backend_allow-app.yaml

mod status_test;

pub use status_test::RefGrantStatusTestSuite;
