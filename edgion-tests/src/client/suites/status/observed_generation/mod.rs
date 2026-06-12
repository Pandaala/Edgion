//! ObservedGeneration Status Test Suite
//!
//! Tests that Gateway and GatewayClass status conditions include the correct
//! `observedGeneration` field matching `metadata.generation`.
//!
//! Required config files (in edgion-tests/conf/Status/ObservedGeneration/):
//! - GatewayClass_observed-gen-class.yaml           (with metadata.generation: 5)
//! - Gateway_edgion-test_observed-gen-gateway.yaml  (with metadata.generation: 3)

pub mod dynamic;
mod status_test;

pub use dynamic::{HighFreqFinalTestSuite, HighFreqInitialTestSuite};
pub use status_test::ObservedGenerationStatusTestSuite;
