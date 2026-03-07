//! ACME Controller-side modules
//!
//! - `acme_client` - ACME protocol client wrapping instant-acme
//! - `dns_provider` - DNS-01 challenge providers (Cloudflare, AliDNS)
//! - `service` - Background orchestrator for certificate issuance/renewal

pub mod acme_client;
pub mod dns_provider;
pub mod service;

pub use service::{notify_resource_changed, start_acme_service, stop_acme_service};
