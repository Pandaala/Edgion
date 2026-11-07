---
knowledge_type: regression-guards
---

# Failover / EdgionConfigData write TOCTOU fixed with a shared per-resource lock (fr-capi-01, fr-capi-02)

<!-- Canonical semantic decision; related findings update this page. -->

---
status: fixed
finding_id: fr-capi-01, fr-capi-02
closed: 2026-06-01
---

## Current source-command contract (2026-10-30)

The preview-bound ClusterRegionSetting command supersedes the historical helper
and its three-attempt rebase. `apply_setting_failover` retains the shared API
resource lock, but requires a canonical source fingerprint before idempotence or
mutation. It protects unversioned sources too. A changed source returns conflict;
there is no automatic retry against a different preview generation. Generic PUT
keeps its independently documented opt-in If-Match behavior.

`CenterApi::update_one_if_source` uses one bounded Kubernetes read and native
resourceVersion replace, an etcd modification-revision transaction, or the existing
filesystem striped lock plus a read-time fingerprint comparison. Filesystem source
CAS holds its lock and scan permit through the complete blocking operation. Existing
replacement and unlink owners also retain the same lock through actual completion,
so cancellation cannot release it while an old rename/unlink is still pending.
The registry path is pinned and revision changes are rejected before commit.
A cancellation check can prevent an unstarted commit; it cannot undo a started
rename, so timeout means unknown persistence. External file editors are not fenced.

Fresh failover resourceVersion allocation skips a value equal to the current source
version. Readiness reconstructs the startup floor but alone cannot prevent equality
with an out-of-band version introduced after startup.

The API lock map is retained as the existing mechanism; this task establishes no
new hard cardinality or eviction bound for it. Do not repeat the historical
assumption that a count of identities ever seen is itself a fixed memory bound.

## Historical rationale (superseded where it describes the old command)

The remainder records the 2026-06/07 findings and their then-current callers.
The current contract above replaces its unconditional unversioned failover,
three-attempt retry and old helper references.

## Conclusion

`patch_edgion_config_data_failover` (`region_route_handlers.rs`) had an unguarded
read-modify-write: it read a whole EdgionConfigData resource from storage, mutated one
region's `failoverTo` in memory, then wrote the whole resource back. Two concurrent
failover requests for the same `namespace/name` (even targeting different regions)
each read the same pre-modification snapshot, and the second write silently discarded
the first (last-writer-wins) while both callers still got `{"success":true,"modified":true}`.

Fixed (fr-capi-01) by serializing the whole get -> patch -> write with a per-resource
`tokio::sync::Mutex`, held for the duration of the helper. Then (fr-capi-02) the same
lock was extended to the generic `update_namespaced` PUT's EdgionConfigData branch —
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
- **As of this fix (2026-06-01)** neither backend provided optimistic concurrency on
  this path: the K8s `update_one` (`kubernetes/storage.rs`) re-fetched the *current*
  `resourceVersion` immediately before `replace` and stamped it on, so it did **not**
  carry the handler's read-time version; the FileSystem backend had no version check
  at all. The TOCTOU window therefore existed on both backends and could not be closed
  by relying on the K8s API server — an in-process lock was the only protection.
  This is no longer the current state; see [Superseded in Part](#superseded-in-part--cas-landed-later-2026-07-28) below.
- The reconcile/watch loop is **not** a concurrent spec writer: K8s reconcile
  persists via `patch_status` on the status subresource
  (`kubernetes/resource_controller.rs`) and the FileSystem reconcile writes a separate
  `.status` sidecar via `write_status_value` (`file_system/resource_controller.rs`).
  Neither rewrites the spec, so the only concurrent *spec* writers are the failover
  handlers and the generic PUT — both now hold the shared lock.
- Lock key includes `kind` so unrelated kinds sharing a `namespace/name` never falsely
  contend. Failover always passes `EdgionConfigData`; the PUT passes its own `kind`, so
  the failover handler and the PUT's EdgionConfigData branch share the same key.
- The map (`RESOURCE_WRITE_LOCKS`, a module-level `LazyLock<DashMap<..>>`) is bounded
  by the number of distinct resources ever locked (tiny), each entry an empty
  `Mutex<()>`, so no eviction is needed. The DashMap shard guard from `entry(..)` is
  dropped before `.lock_owned().await`, so it is never held across the await.
- `tokio::sync::Mutex` (not `std::sync::Mutex`) because the guard is held across the
  `.await` storage I/O of `center_get_resource` / `update_one`. `lock_owned` returns
  an `OwnedMutexGuard` so the helper can hand the guard back to the caller.
- The PUT acquires the lock only for its EdgionConfigData branch (the only kind with a
  failover RMW competitor); other kinds gain nothing from locking, so they are left
  lock-free to keep blast radius minimal.

## Superseded in Part — CAS landed later (2026-07-28)

The backends now **do** reject a stale write on this path. `update_one_if_version`
threads a caller-supplied `resourceVersion` precondition to the writer, and all three
backends enforce it via `ensure_expected_resource_version_value`
(`conf_mgr/conf_center/common.rs`): the FileSystem compares inside its per-key
`update_lock`, etcd compares in-process and *then* fuses the write into a
`txn_put_if_mod_revision` (that transaction's compare is on etcd's own `mod_revision`,
guarding the backend's internal get→put window — a different value guarding a different
window), and Kubernetes compares the fetched object immediately before `replace`
(`kubernetes/storage.rs`). `patch_edgion_config_data_failover` supplies one it read
itself and retries a `Conflict` twice (three attempts total); `update_namespaced`
forwards whatever the client sent — its `If-Match` or body `metadata.resourceVersion`,
which is often nothing.

So "the backends ignore `resourceVersion`" must no longer be repeated, and the lock's
rationale has moved rather than disappeared. What is still true is the short-circuit:
`ensure_expected_resource_version_value` returns `Ok(())` when the caller supplies no
version, so a precondition-free write stays an unconditional last-writer-wins replace.
That opt-in contract is deliberate — see
[`../architecture/admin-api-write-precondition-opt-in.md`](../../admin-api/intentional-design/admin-api-write-precondition-opt-in.md).

The lock therefore stays load-bearing for what the precondition does not cover:

1. No lock-taking writer can land inside another's get → patch → write window.
   Not just a precondition-free replace — a conditional PUT that wins, or a second
   failover request, does the same thing: it turns the failover in progress into a CAS
   conflict and, under sustained contention, exhausts its three attempts and fails the
   request. CAS prevents the *loss*, not the *conflict*, and the retry budget is finite.
   Scope this claim to the two lock-takers. `delete_namespaced`
   (`delete_one_if_version`) and `wipe_all` (`delete_one`) take no lock and can still
   land mid-window; they surface as `NotFound` and fail the request (HTTP 500), which
   is not a lost update.
2. Where the stored object carries no `metadata.resourceVersion` at all, the failover
   has nothing to send and its own write is unconditional (`region_route_handlers.rs`
   reads the version off the snapshot it patched and passes `None` when the pointer is
   absent). Reachable only outside Kubernetes, which assigns a version server-side: a
   hand-authored YAML file in the FileSystem `conf_dir` (nothing normalizes it —
   `bump_resource_version_floor` only raises the in-memory allocator floor, it never
   writes back), or an object put into etcd out of band. On that path the lock is the
   sole protection against a genuine silent lost update, exactly as it was in 2026-06.
   The exposure self-heals: it lasts only until the first Admin API write to that
   object stamps a version — `set_fresh_resource_version` on the failover path,
   `update_resource_version` (via `parse_resource_and_update_version`) on the generic
   PUT — which FileSystem and etcd then persist through `merge_operator_update_yaml`.

It does **not** protect a caller that sends a stale unconditional write after another
writer committed; detecting that is the caller's job. The `api/resource_locks.rs`
module doc was corrected to say all of this (finding `new_work-19`, closed 2026-07-28);
reject any re-report claiming either that the backends never check `resourceVersion` or
that CAS has made the lock obsolete.

## Fix Suggestions Not Accepted

- "Rely on `resourceVersion` optimistic concurrency / CAS in the storage backend"
  — declined at the time, and since **superseded**. The original finding assumed the
  K8s backend already rejected a stale write via `resourceVersion`; in 2026-06 it did
  not (see Core Rationale). A real `compare_and_swap_one` round-tripping the read-time
  version was judged the general backend-correct fix but a wider `ConfCenter` trait
  change; the in-process lock was sufficient for the single-process controller and is
  what shipped. That wider change has since landed as `update_one_if_version` — it
  complements the lock rather than replacing it (see Superseded in Part).
- "Single global failover mutex" — workable/simplest, rejected for per-key so distinct
  resources don't block each other; per-key is the cleaner expression of the invariant.
- "Lock field on `AdminState`" — rejected to keep blast radius minimal: it would touch
  all 6 `AdminState` construction sites (1 real + 5 test). A shared module-level static
  has identical semantics (controller is single-process, `AdminState` is effectively a
  singleton) while keeping the lock in one place both handlers can reach.
- "Lock every kind in `update_namespaced`" — rejected: only EdgionConfigData has a
  second writer (failover) doing a read-modify-write on the same resource via a
  separate endpoint; locking the other kinds adds contention for no correctness gain.

## Re-evaluation Triggers

Re-open this decision only if:

- A cross-process / multi-replica controller writer appears (the in-process lock no
  longer suffices — needs a storage-level CAS / `resourceVersion` guard).
- A new endpoint replaces or read-modify-writes the same EdgionConfigData without going
  through `resource_locks::lock_resource` (re-opens the interleaving window).
- `patch_edgion_config_data_failover` starts mutating across multiple resources in one
  call (single per-key lock no longer suffices; lock-ordering required).
- ~~A real `compare_and_swap_one` is added to the `ConfCenter` trait (this ad-hoc lock
  can then be replaced by it).~~ This happened (`update_one_if_version`) and was
  re-evaluated: the lock is **not** replaced by it, because the precondition is opt-in.
  See Superseded in Part.

## Reference Cases

- Original findings: `tasks/bug/fr-capi-01-failover-toctou.md` (closed),
  `tasks/todo/fr-capi-02-failover-vs-generic-put-residual-race.md` (closed by the
  same change).
- Code: `edgion-controller/src/api/resource_locks.rs` (`lock_resource`),
  `region_route_handlers.rs` (`patch_edgion_config_data_failover`),
  `namespaced_handlers.rs` (`update_namespaced` EdgionConfigData branch).
- Sibling concurrency TOCTOU fixes: `backends/set-service-config-toctou-race.md`,
  `backends/warm-cell-toctou-stale-overwrite.md`, `concurrency/superseded/halfopen-probe-toctou-cas.md` (historical).

## Resolution references

- `new_work-19` — 2026-07-28 (comment-only).
