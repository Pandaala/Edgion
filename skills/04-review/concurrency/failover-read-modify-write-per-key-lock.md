# Failover / PluginMetaData write TOCTOU fixed with a shared per-resource lock (fr-capi-01, fr-capi-02)

<!-- canonical decision record; one finding = one file -->

---
status: fixed
finding_id: fr-capi-01, fr-capi-02
closed: 2026-06-01
---

## Conclusion

`patch_plugin_metadata_failover` (`region_route_handlers.rs`) had an unguarded
read-modify-write: it read a whole PluginMetaData resource from storage, mutated one
region's `failoverTo` in memory, then wrote the whole resource back. Two concurrent
failover requests for the same `namespace/name` (even targeting different regions)
each read the same pre-modification snapshot, and the second write silently discarded
the first (last-writer-wins) while both callers still got `{"success":true,"modified":true}`.

Fixed (fr-capi-01) by serializing the whole get -> patch -> write with a per-resource
`tokio::sync::Mutex`, held for the duration of the helper. Then (fr-capi-02) the same
lock was extended to the generic `update_namespaced` PUT's PluginMetaData branch —
the other in-process writer that replaces the whole resource via `update_one` (and the
path the Center region-route sync fan-out lands on) — so failover-vs-PUT can no longer
interleave either. The lock lives in the shared `api::resource_locks` module
(`lock_resource(kind, namespace, name) -> OwnedMutexGuard<()>`), keyed by
`kind/namespace/name`.

## Core Rationale

- The resource is **not** a long-lived in-memory object: `center_get_resource`
  re-deserializes a fresh `serde_json::Value` from storage on every call, so a lock
  "inside the resource" is meaningless — concurrent requests hold independent Values.
  The lock must live in a cross-request, per-resource-identity structure.
- Neither backend provides optimistic concurrency on this path: the K8s
  `update_one` (`kubernetes/storage.rs`) re-fetches the *current* `resourceVersion`
  immediately before `replace` and stamps it on, so it does **not** carry the
  handler's read-time version; the FileSystem backend has no version check at all.
  The TOCTOU window therefore exists on both backends and cannot be closed by
  relying on the K8s API server — an in-process lock is the only protection.
- The reconcile/watch loop is **not** a concurrent spec writer: K8s reconcile
  persists via `patch_status` on the status subresource
  (`kubernetes/resource_controller.rs`) and the FileSystem reconcile writes a separate
  `.status` sidecar via `write_status_value` (`file_system/resource_controller.rs`).
  Neither rewrites the spec, so the only concurrent *spec* writers are the failover
  handlers and the generic PUT — both now hold the shared lock.
- Lock key includes `kind` so unrelated kinds sharing a `namespace/name` never falsely
  contend. Failover always passes `PluginMetaData`; the PUT passes its own `kind`, so
  the failover handler and the PUT's PluginMetaData branch share the same key.
- The map (`RESOURCE_WRITE_LOCKS`, a module-level `LazyLock<DashMap<..>>`) is bounded
  by the number of distinct resources ever locked (tiny), each entry an empty
  `Mutex<()>`, so no eviction is needed. The DashMap shard guard from `entry(..)` is
  dropped before `.lock_owned().await`, so it is never held across the await.
- `tokio::sync::Mutex` (not `std::sync::Mutex`) because the guard is held across the
  `.await` storage I/O of `center_get_resource` / `update_one`. `lock_owned` returns
  an `OwnedMutexGuard` so the helper can hand the guard back to the caller.
- The PUT acquires the lock only for its PluginMetaData branch (the only kind with a
  failover RMW competitor); other kinds gain nothing from locking, so they are left
  lock-free to keep blast radius minimal.

## Fix Suggestions Not Accepted

- "Rely on `resourceVersion` optimistic concurrency / CAS in the storage backend"
  — the original finding assumed the K8s backend already rejects a stale write via
  `resourceVersion`; it does not (see Core Rationale). A real `compare_and_swap_one`
  round-tripping the read-time version is the general backend-correct fix but a wider
  `ConfCenter` trait change; the in-process lock is sufficient for the single-process
  controller and is what shipped. (Re-open if a cross-process writer ever appears.)
- "Single global failover mutex" — workable/simplest, rejected for per-key so distinct
  resources don't block each other; per-key is the cleaner expression of the invariant.
- "Lock field on `AdminState`" — rejected to keep blast radius minimal: it would touch
  all 6 `AdminState` construction sites (1 real + 5 test). A shared module-level static
  has identical semantics (controller is single-process, `AdminState` is effectively a
  singleton) while keeping the lock in one place both handlers can reach.
- "Lock every kind in `update_namespaced`" — rejected: only PluginMetaData has a
  second writer (failover) doing a read-modify-write on the same resource via a
  separate endpoint; locking the other kinds adds contention for no correctness gain.

## Re-evaluation Triggers

Re-open this decision only if:

- A cross-process / multi-replica controller writer appears (the in-process lock no
  longer suffices — needs a storage-level CAS / `resourceVersion` guard).
- A new endpoint replaces or read-modify-writes the same PluginMetaData without going
  through `resource_locks::lock_resource` (re-opens the interleaving window).
- `patch_plugin_metadata_failover` starts mutating across multiple resources in one
  call (single per-key lock no longer suffices; lock-ordering required).
- A real `compare_and_swap_one` is added to the `ConfCenter` trait (this ad-hoc lock
  can then be replaced by it).

## Reference Cases

- Original findings: `tasks/bug/fr-capi-01-failover-toctou.md` (closed),
  `tasks/todo/fr-capi-02-failover-vs-generic-put-residual-race.md` (closed by the
  same change).
- Code: `edgion-controller/src/api/resource_locks.rs` (`lock_resource`),
  `region_route_handlers.rs` (`patch_plugin_metadata_failover`),
  `namespaced_handlers.rs` (`update_namespaced` PluginMetaData branch).
- Sibling concurrency TOCTOU fixes: `backends/set-service-config-toctou-race.md`,
  `backends/warm-cell-toctou-stale-overwrite.md`, `concurrency/halfopen-probe-toctou-cas.md`.
