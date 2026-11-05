//! Per-resource serialization for the validated setting command and generic
//! EdgionConfigData replacement. Both use the same kind/namespace/name key.
//!
//! The setting command requires a canonical source fingerprint, including for
//! unversioned sources, and never rebases a stale preview. Generic resource PUT
//! retains its opt-in resourceVersion precondition and last-writer-wins contract
//! when no precondition is supplied. Neither contract treats this process lock
//! as cross-process authorization or storage-level compare-and-swap.
//!
//! Storage still owns atomicity: Kubernetes uses native version replacement,
//! etcd compares modification revision, and filesystem replacement/unlink retains
//! its own shared lock through actual blocking work even after caller cancellation.
//! The API lock reduces interleaving with the generic EdgionConfigData writer;
//! cancellation releases it, while an already started filesystem commit remains
//! protected by the storage lock. External file editors are not atomically fenced.
//!
//! Current users are `region_route_handlers::apply_setting_failover` and the
//! EdgionConfigData branch of `namespaced_handlers::update_namespaced`.
//! See `skills/04-review/concurrency/regression-guards/failover-read-modify-write-per-key-lock.md`.

use dashmap::DashMap;
use std::sync::{Arc, LazyLock};
use tokio::sync::{Mutex, OwnedMutexGuard};

static RESOURCE_WRITE_LOCKS: LazyLock<DashMap<String, Arc<Mutex<()>>>> = LazyLock::new(DashMap::new);

/// Acquire the per-resource write lock for `kind/namespace/name`, held until the
/// returned guard is dropped. Hold it across the whole read-modify-write (or
/// replace) so concurrent writers to the same resource cannot interleave.
///
/// `kind` is part of the key so unrelated kinds that happen to share a
/// `namespace/name` never falsely contend.
pub(super) async fn lock_resource(kind: &str, namespace: &str, name: &str) -> OwnedMutexGuard<()> {
    let key = format!("{}/{}/{}", kind, namespace, name);
    // The DashMap shard guard from `entry(..)` is dropped at the end of this
    // statement (before `.lock_owned().await`), so it is never held across the
    // await — only the cloned `Arc<Mutex<()>>` outlives it.
    let lock = RESOURCE_WRITE_LOCKS.entry(key).or_default().clone();
    lock.lock_owned().await
}

#[cfg(test)]
#[path = "../../tests/unit/api/resource_locks_test.rs"]
mod tests;
