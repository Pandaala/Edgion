//! Shared lifecycle interface for LinkSys providers that use generic dispatch.
//!
//! Dispatch maps resource events to three lifecycle operations:
//! - **add / update** — calls `init` on the new instance
//! - **update** — calls `shutdown` on the old instance only when
//!   `SHUTDOWN_ON_REPLACE = true`
//! - **remove** — always calls `shutdown` regardless of `SHUTDOWN_ON_REPLACE`
//!
//! Redis, Etcd, Elasticsearch, and Kafka use this interface and shut down
//! replaced instances. Gateway Webhook publication deliberately does not
//! implement this trait: it prepares candidates and commits them through the
//! sequence-checked `WebhookManager` before updating runtime bookkeeping.

use anyhow::Result;
use async_trait::async_trait;

#[async_trait]
pub trait LinkSysLifecycle: Send + Sync + 'static {
    /// Called on add and update — bring the instance online.
    async fn init(&self) -> Result<()> {
        Ok(())
    }

    /// Called on remove; on update only if `SHUTDOWN_ON_REPLACE` is true.
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    /// On update, whether the previous instance must be shut down explicitly.
    ///   - `true` (default): the instance owns connections or background tasks.
    ///   - `false`: `init` provides complete replacement semantics.
    const SHUTDOWN_ON_REPLACE: bool = true;
}
