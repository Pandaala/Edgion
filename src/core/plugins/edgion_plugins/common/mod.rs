//! Common utilities shared across EdgionPlugins
//!
//! Provides shared infrastructure for plugins that need to make HTTP requests
//! to external services (e.g., ForwardAuth, OPA, Webhook).

pub mod http_client;

pub use http_client::{get_http_client, is_hop_by_hop, HOP_BY_HOP_HEADERS};
