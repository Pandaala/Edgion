//! Webhook service management for LinkSys Webhook resources.
//!
//! Provides health-aware webhook invocation for KeyGet::Webhook resolution.
//! Runtime state is managed by WebhookManager, which is populated via the
//! LinkSysStore ConfHandler when LinkSys Webhook resources are synced.

pub mod client;
pub mod health;
pub mod manager;
pub mod resolver;
pub mod runtime;

// Re-export key types for external consumers
pub use client::WebhookLinkClient;
pub use manager::{get_webhook_manager, StaticWebhookManagerAdapter, WebhookManager, WebhookManagerOps};
pub use resolver::resolve_webhook_key;
