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
//! - BackendTLSPolicy_edgion-test_system-ca.yaml
//! - BackendTLSPolicy_edgion-test_both-invalid.yaml
//! - BackendTLSPolicy_edgion-test_neither-invalid.yaml
//! - ConfigMap_edgion-test_test-ca-configmap.yaml
//! - BackendTLSPolicy_edgion-test_configmap-ca.yaml
//! - BackendTLSPolicy_edgion-test_configmap-ca-missing.yaml

mod status_test;

pub use status_test::BackendTlsStatusTestSuite;
