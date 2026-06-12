//! ReferenceGrant and Status System Test Suite
//!
//! Tests cross-namespace reference validation and status updates.
//!
//! Required config files (in examples/test/conf/Status/RefGrant/):
//! - Service_backend_cross-ns-svc.yaml
//! - EndpointSlice_backend_cross-ns-svc.yaml
//! - HTTPRoute_app_cross-ns-route.yaml      (route namespace: edgion-default)
//! - HTTPRoute_app_cross-ns-denied.yaml     (route namespace: edgion-default)
//! - HTTPRoute_app_multi-parent.yaml        (route namespace: edgion-default)
//! - ReferenceGrant_backend_allow-app.yaml
//!
//! TCPRoute cross-namespace sub-suite (in examples/test/conf/Status/RefGrant/TCPRouteCrossNsSvc/):
//! - TCPRoute_app_tcp-cross-ns-allowed.yaml    (ns=app, has ReferenceGrant)
//! - TCPRoute_app2_tcp-cross-ns-denied.yaml    (ns=app2, no ReferenceGrant)
//! - ReferenceGrant_backend_allow-app-tcproute.yaml

mod status_test;
mod tcp_cross_ns;
mod tls_cross_ns_svc;

pub use status_test::RefGrantStatusTestSuite;
pub use tcp_cross_ns::TcpRouteCrossNsSvcTestSuite;
pub use tls_cross_ns_svc::TlsRouteCrossNsSvcTestSuite;
