//! TLSRoute Status Test Suite
//!
//! Tests the status system for TLSRoute resources:
//! 1. Route with non-existent backend → Accepted=True, ResolvedRefs=False(BackendNotFound)
//! 2. Route with invalid backend kind → Accepted=True, ResolvedRefs=False(InvalidKind)
//!
//! Required config files (in edgion-tests/conf/Status/TLSRoute/):
//! - Gateway_edgion-test_tlsroute-status-gateway.yaml
//! - TLSRoute_edgion-default_tls-nonexistent-backend.yaml
//! - TLSRoute_edgion-default_tls-invalid-kind.yaml

mod status_test;

pub use status_test::TlsRouteStatusTestSuite;
