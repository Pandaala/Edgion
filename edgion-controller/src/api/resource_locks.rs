//! Per-resource serialization locks for admin write paths.
//!
//! Some admin endpoints mutate a single stored resource through a non-atomic
//! read-modify-write (read the whole object, change a field in memory, write the
//! whole object back) or a whole-object replace. Two such writers targeting the
//! same resource can interleave on the underlying store — neither the FileSystem
//! nor the Kubernetes backend rejects a stale write via `resourceVersion` on this
//! path — so the second write silently discards the first (last-writer-wins).
//!
//! `lock_resource` hands out a per-`kind/namespace/name` async mutex that such
//! writers hold for the duration of their read-modify-write / replace, so writes
//! to the same resource run one at a time. Current users:
//!   - `region_route_handlers::patch_edgion_config_data_failover` (failover RMW)
//!   - `namespaced_handlers::update_namespaced` (generic EdgionConfigData PUT,
//!     which the Center region-route sync also routes through)
//! See `skills/04-review/concurrency/failover-read-modify-write-per-key-lock.md`.
//!
//! The map is keyed by `kind/namespace/name` and is bounded by the number of
//! distinct resources ever locked; each entry is an empty `Mutex<()>`, so no
//! eviction is needed. The lock is in-process only (one controller process owns
//! its store); it is not a cross-process / cluster-wide lock.

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
mod tests {
    use super::*;

    /// Acquire the lock for an identity via `try_lock_owned` (no wall-clock timing).
    /// `None` means "currently held" (contended); `Some(_)` means "acquired".
    fn try_acquire(kind: &str, ns: &str, name: &str) -> Option<OwnedMutexGuard<()>> {
        let key = format!("{}/{}/{}", kind, ns, name);
        RESOURCE_WRITE_LOCKS
            .entry(key)
            .or_default()
            .clone()
            .try_lock_owned()
            .ok()
    }

    #[tokio::test]
    async fn same_identity_shares_one_lock_distinct_identity_does_not() {
        // Use unique identities so this test is insensitive to entries left in the
        // process-global map by other tests, and deterministic regardless of the
        // test runtime flavor (no wall-clock timeouts).
        let g1 = lock_resource("EdgionConfigData", "ns-rl-test", "pm").await;

        // Same kind/ns/name must serialize: while g1 is held, a second acquisition
        // for the same identity must fail (None).
        assert!(
            try_acquire("EdgionConfigData", "ns-rl-test", "pm").is_none(),
            "second lock on the same identity must be contended while the first is held"
        );

        // A different name, namespace, or kind must NOT contend (acquires immediately).
        assert!(
            try_acquire("EdgionConfigData", "ns-rl-test", "other").is_some(),
            "distinct name must not contend"
        );
        assert!(
            try_acquire("EdgionConfigData", "other-ns-rl-test", "pm").is_some(),
            "distinct namespace must not contend"
        );
        assert!(
            try_acquire("Service", "ns-rl-test", "pm").is_some(),
            "distinct kind must not contend"
        );

        drop(g1);
        // Once released, the identity is acquirable again.
        assert!(
            try_acquire("EdgionConfigData", "ns-rl-test", "pm").is_some(),
            "lock must be reacquirable after release"
        );
    }
}
