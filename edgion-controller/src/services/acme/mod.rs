//! ACME Controller-side modules
//!
//! - `acme_client` - ACME protocol client wrapping instant-acme
//! - `service` - Background orchestrator for certificate issuance/renewal

pub mod acme_client;
mod secret_io;
pub mod service;

pub use service::{notify_resource_changed, start_acme_service, stop_acme_service};
