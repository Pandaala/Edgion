//! Per-resource serialization locks for admin write paths.
//!
//! Some admin endpoints mutate a single stored resource through a non-atomic
//! read-modify-write (read the whole object, change a field in memory, write the
//! whole object back) or a whole-object replace.
//!
//! The lock is not a substitute for the `resourceVersion` precondition, and it is
//! not here because the backends ignore one: the FileSystem, etcd, and Kubernetes
//! writers all enforce it through `ensure_expected_resource_version_value`
//! (`conf_mgr/conf_center/common.rs`), which returns `Conflict` on mismatch. What
//! they do not do is *require* it — that check short-circuits to `Ok(())` when the
//! caller supplies no version, so a precondition-free write remains an
//! unconditional last-writer-wins replace. That contract is deliberate; see
//! `skills/04-review/admin-api/intentional-design/admin-api-write-precondition-opt-in.md`.
//!
//! `lock_resource` hands out a per-`kind/namespace/name` async mutex that such
//! writers hold for the duration of their read-modify-write / replace, so writes
//! to the same resource run one at a time. What that buys on top of the
//! precondition:
//!   - no lock-taking writer can land inside another's window: a precondition-free
//!     replace, a conditional one that wins, or a second failover request would
//!     each turn the failover in progress into a CAS conflict and, under sustained
//!     contention, exhaust its three attempts. (Writers that do not take the lock —
//!     `delete_namespaced`, `wipe_all` — still can; they surface as `NotFound` and
//!     fail the request, which is not a lost update.)
//!   - where the stored object carries no `metadata.resourceVersion` at all — a
//!     hand-authored file in the FileSystem `conf_dir`, or an object written to
//!     etcd out of band; Kubernetes assigns one server-side — the failover has no
//!     precondition to send and its own write is unconditional. There the lock is
//!     the only thing standing between two writers and a silently lost update.
//!     It self-heals: the exposure lasts only until the first Admin API write
//!     stamps a version (`set_fresh_resource_version` on the failover path,
//!     `update_resource_version` on the generic PUT).
//!
//! It does not protect a caller that simply sends a stale unconditional write
//! after another writer committed — that is last-writer-wins by design, and
//! detecting it is the caller's job (send the precondition from its own `GET`).
//!
//! Current users:
//!   - `region_route_handlers::patch_edgion_config_data_failover` (failover RMW)
//!   - `namespaced_handlers::update_namespaced` (generic EdgionConfigData PUT,
//!     which the Center region-route sync also routes through)
//!
//! See `skills/04-review/concurrency/regression-guards/failover-read-modify-write-per-key-lock.md`.
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
#[path = "../../tests/unit/api/resource_locks_test.rs"]
mod tests;
