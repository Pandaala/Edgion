// ACME Service Integration Tests
//
// Validates the full ACME certificate issuance flow against Pebble
// (Let's Encrypt's official ACME test server).
//
// Prerequisites:
//   cd examples/test/conf/Services/acme/pebble
//   docker compose up -d
//
// These tests exercise:
//   - AcmeClient account creation & credential restore
//   - DNS-01 full certificate issuance flow
//   - DNS-01 multi-domain (SAN) certificates
//   - HTTP-01 flow (when container-to-host connectivity is available)

mod acme;

pub use acme::AcmeTestSuite;
