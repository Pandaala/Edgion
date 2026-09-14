// ACME Service Integration Tests
//
// Validates ACME account creation and credential restoration against Pebble
// (Let's Encrypt's official ACME test server).
//
// Prerequisites:
//   cd edgion-tests/integration/conf/Services/acme/pebble
//   docker compose up -d
//
// These tests exercise:
//   - AcmeClient account creation & credential restore

mod acme;
mod gatewayref_ownership;

pub use acme::AcmeTestSuite;
pub use gatewayref_ownership::{AcmeGatewayRefOwnershipPhase, AcmeGatewayRefOwnershipTestSuite};
