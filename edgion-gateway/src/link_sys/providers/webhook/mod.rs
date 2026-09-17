//! Webhook service management for LinkSys Webhook resources.
//!
//! Provides health-aware webhook invocation for the WebhookKeyGet plugin.
//! Runtime state is managed by WebhookManager, which is populated via the
//! LinkSysStore ConfHandler when LinkSys Webhook resources are synced.

pub(crate) mod apply;
pub mod client;
pub mod manager;
pub mod resolver;
pub mod runtime;

// Re-export key types for external consumers
pub(crate) use apply::fingerprint_webhook;
pub use client::WebhookLinkClient;
pub use manager::{
    configure_webhook_global_max, get_webhook_call_handle, get_webhook_global_max, get_webhook_manager,
    WebhookCallHandle, WebhookManager,
};
pub use resolver::resolve_webhook_key;
