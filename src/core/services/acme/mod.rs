//! ACME certificate management service
//!
//! Provides automatic TLS certificate issuance and renewal via the ACME protocol (RFC 8555).
//!
//! ## Architecture
//!
//! - **ctrl** (Controller side): Watches EdgionAcme CRDs, runs the ACME protocol,
//!   creates/updates K8s Secrets and EdgionTls resources.
//! - **gw** (Gateway side): Receives EdgionAcme resources with active challenge tokens,
//!   serves HTTP-01 challenge responses in `early_request_filter`.

pub mod ctrl;
pub mod gw;

// Re-export for convenience
pub use ctrl::{notify_resource_changed, start_acme_service, stop_acme_service};
pub use gw::create_acme_handler;
