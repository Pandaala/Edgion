//! Shared authentication primitives.
//!
//! After the controller's Admin API moved to a token-only model, this module
//! keeps only the primitives still shared across crates: the JWKS/discovery
//! provider used by the gateway OIDC plugin, and the canonical public-path set
//! used by the controller's authorization layer.

pub mod jwks_provider;
pub mod public_paths;
