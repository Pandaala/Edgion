//! Webhook service management for LinkSys Webhook resources.
//!
//! Provides health-aware webhook invocation for KeyGet::Webhook resolution.
//! Runtime state is managed by WebhookManager, which is populated via the
//! LinkSysStore ConfHandler when LinkSys Webhook resources are synced.

pub mod call;
pub mod client;
pub mod health;
pub mod manager;
pub mod resolver;
pub mod runtime;

// Re-export key types for external consumers
pub use call::{webhook_call, WebhookCallError, WebhookCallRequest, WebhookCallResult};
pub use client::WebhookLinkClient;
pub use manager::{
    configure_webhook_global_max, get_webhook_call_handle, get_webhook_global_max, get_webhook_manager,
    StaticWebhookManagerAdapter, WebhookCallHandle, WebhookManager, WebhookManagerOps,
};
pub use resolver::resolve_webhook_key;
