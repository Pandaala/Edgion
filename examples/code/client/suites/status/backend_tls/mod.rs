//! BackendTLSPolicy Status Test Suite
//!
//! Tests BackendTLSPolicy validation and status conditions:
//! 1. wellKnownCACertificates: System → Accepted=True
//! 2. Both caCertificateRefs and wellKnownCACertificates → Accepted=False (mutually exclusive)
//! 3. Neither caCertificateRefs nor wellKnownCACertificates → Accepted=False (must specify one)
//!
//! Required config files (in examples/test/conf/Status/BackendTLSPolicy/):
//! - BackendTLSPolicy_edgion-test_system-ca.yaml
//! - BackendTLSPolicy_edgion-test_both-invalid.yaml
//! - BackendTLSPolicy_edgion-test_neither-invalid.yaml

mod status_test;

pub use status_test::BackendTlsStatusTestSuite;
