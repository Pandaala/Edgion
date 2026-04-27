//! BackendTLSPolicy Status Test Suite
//!
//! Tests BackendTLSPolicy validation and status conditions:
//! 1. wellKnownCACertificates: System → Accepted=True
//! 2. Both caCertificateRefs and wellKnownCACertificates → Accepted=False (mutually exclusive)
//! 3. Neither caCertificateRefs nor wellKnownCACertificates → Accepted=False (must specify one)
//! 4. ConfigMap caCertificateRef with valid ConfigMap → Accepted=True, ResolvedRefs=True
//! 5. ConfigMap caCertificateRef with missing ConfigMap → ResolvedRefs=False
//!
//! Required config files (in examples/test/conf/Status/BackendTLSPolicy/):
//!
//! Ancestor chain (Gateway + Route + Services — needed to populate status.ancestors):
//! - Gateway_edgion-test_btls-status-gateway.yaml
//! - HTTPRoute_edgion-test_btls-status-route.yaml
//! - Service_edgion-test_test-backend-system-ca.yaml
//! - Service_edgion-test_test-backend-configmap-ca.yaml
//! - Service_edgion-test_test-backend-both.yaml
//! - Service_edgion-test_test-backend-neither.yaml
//! - Service_edgion-test_test-backend-configmap-missing.yaml
//! - Service_edgion-test_test-backend-tls-system-ca.yaml
//!
//! BackendTLSPolicy resources under test:
//! - BackendTLSPolicy_edgion-test_system-ca.yaml
//! - BackendTLSPolicy_edgion-test_both-invalid.yaml
//! - BackendTLSPolicy_edgion-test_neither-invalid.yaml
//! - BackendTLSPolicy_edgion-test_configmap-ca.yaml
//! - BackendTLSPolicy_edgion-test_configmap-ca-missing.yaml
//!
//! Supporting resources:
//! - ConfigMap_edgion-test_test-ca-configmap.yaml

pub mod dynamic;
mod status_test;

pub use dynamic::{BtlspFlapInitialTestSuite, BtlspFlapInvalidTestSuite, BtlspFlapValidTestSuite};
pub use status_test::BackendTlsStatusTestSuite;
