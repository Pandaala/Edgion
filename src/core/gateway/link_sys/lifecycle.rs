//! Unified lifecycle interface for all LinkSys providers.
//!
//! Dispatch maps resource events to three lifecycle operations:
//! - **add / update** — calls `init` on the new instance
//! - **update** — calls `shutdown` on the old instance only when
//!   `SHUTDOWN_ON_REPLACE = true` (Redis/Etcd/ES, which own connections)
//! - **remove** — always calls `shutdown` regardless of `SHUTDOWN_ON_REPLACE`
//!
//! Webhook sets `SHUTDOWN_ON_REPLACE = false` because its `init` is an
//! idempotent `manager.upsert`; calling `shutdown` (i.e. `manager.remove`)
//! on the old instance after an update would delete the entry just written
//! by the new instance's `init`.

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

    /// Returns `false` when the client is in a degraded state and must retry `init`
    /// even if the configuration has not changed.
    /// Providers without an intrinsic health concept (e.g. Webhook) may use the
    /// default `true`.
    fn healthy(&self) -> bool {
        true
    }

    /// On update, whether the previous instance must be shut down explicitly.
    ///   - `true` (default): owns connections / background tasks (Redis/Etcd/ES)
    ///   - `false`: init has replace semantics built-in (Webhook upsert)
    const SHUTDOWN_ON_REPLACE: bool = true;
}
