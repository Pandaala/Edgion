---
name: warm-cell-toctou-stale-overwrite
description: LB preload warm_cell now seeds the cell via hydrate_if_empty (double-checked lock shared with replace_backends) instead of a bare load().is_empty() + replace_backends. Reject re-reports claiming the preload check-then-write is a TOCTOU that can overwrite a fresher backend list with a stale snapshot.
knowledge_type: superseded
---

# backends-F005 — `warm_cell` startup hydration is now an atomic `hydrate_if_empty`

**Status:** superseded · 2026-09-02
**Severity:** Medium (BadLogic) — non-atomic check+write during preload could overwrite a concurrent EP-sync update with a stale snapshot

## Conclusion

The preload, `warm_cell`, hydration, and global Service selector-cell cache were deleted when
Service/EndpointSlice reconciliation moved to eager selector compilation in
`edgion-gateway/src/backends/discovery/coordinator.rs`. Each serialized source transaction builds
the complete affected projections and publishes one joint generation, so the former startup
check-then-write path no longer exists.

Re-reports about `warm_cell` or `hydrate_if_empty` are not accepted unless those mechanisms are
reintroduced. Review the Coordinator's single-generation publication and per-Service activation
barriers instead.

## What was wrong (before the fix)

`warm_cell` read the backend list once at the top (`get_backends_for_service`), then did a bare
`cell.backends.load().is_empty()` check followed by `cell.replace_backends(snapshot)` with no
lock held across the two steps. The Tokio runtime (started before `preload_load_balancers` runs)
had live config-sync watch tasks; an EP-sync event arriving between the check and the write called
the former affected-LB rebuild helper, then `replace_backends(fresher)`.
`warm_cell`'s subsequent `replace_backends(stale)` then clobbered the fresher list with the
snapshot taken at the top, leaving the cell one EP-sync behind until the next `replace_backends`.
The harm window is startup-only but reachable (e.g. a rolling deploy finishing right as the
gateway boots): traffic in that window could target already-removed pod IPs.

## Why the fix is correct

`hydrate_if_empty` (`compiled_selector.rs`) is a double-checked lock against `build_lock`: the
fast path returns early if the cell is already non-empty (no lock), and the slow path acquires
`build_lock`, re-checks `is_empty()`, and only then runs the provider read + `ArcSwap::store`.
Because `replace_backends` takes the same lock, the three relevant orderings are all safe:

- A concurrent `replace_backends(fresher)` that wins the lock first leaves the cell non-empty →
  `warm_cell`'s re-check sees non-empty and returns without overwriting.
- A concurrent `replace_backends(fresher)` that loses the lock runs *after* `warm_cell`'s store
  and overwrites with the fresher list (each EP event always calls `replace_backends` after it
  updates the roundrobin store, so the freshest data still wins).
- No concurrent writer → `warm_cell` reads the store inside the lock and seeds the cell.

This is the same atomic-seed pattern already proven on the data-plane path
(the former request selector hydration path). Same TOCTOU-on-a-config-sync-driven-cell defect class as
[backends-F004](../regression-guards/set-service-config-toctou-race.md) and
[backends-F002](../regression-guards/upsert-task-concurrent-handle-leak.md), fixed with the same
"check and write under one shared lock" principle.

## Fix Suggestions Not Accepted

- "Drop the `is_empty()` pre-check and always call `hydrate_if_empty`" — the pre-check avoids
  creating an empty cell in the global `SELECTOR_CACHE` for services that have no EP data; it is
  retained intentionally. (The closure re-reads the store under the lock anyway, so the pre-check
  is purely a cheap skip, not the correctness boundary.)
- "Hold `build_lock` manually in `warm_cell` and inline the check+store" — duplicates the logic
  `hydrate_if_empty` already encapsulates; reusing the shared method keeps preload and the
  data-plane path on one code path.

## Re-evaluation Triggers

Re-open only if:

- `warm_cell` reverts to a bare `load().is_empty()` + `replace_backends` (or any check+write not
  guarded by the cell's `build_lock`).
- `hydrate_if_empty` stops sharing `build_lock` with `replace_backends`, which would reopen the
  interleaving window for every caller (preload and data-plane alike).

## Reference Cases

- backends-F005 (review finding, 2026-05-24)
- backends-F004 / backends-F002 — sibling fixes, same "atomic mutation under a shared lock" principle.
- `discovery/coordinator.rs::ServiceDiscoveryCoordinator` — the replacement serialized
  publication owner.
