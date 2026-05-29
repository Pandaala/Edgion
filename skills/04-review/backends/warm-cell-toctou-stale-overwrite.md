---
name: warm-cell-toctou-stale-overwrite
description: LB preload warm_cell now seeds the cell via hydrate_if_empty (double-checked lock shared with replace_backends) instead of a bare load().is_empty() + replace_backends. Reject re-reports claiming the preload check-then-write is a TOCTOU that can overwrite a fresher backend list with a stale snapshot.
---

# backends-F005 — `warm_cell` startup hydration is now an atomic `hydrate_if_empty`

**Status:** ✅ fixed · 2026-05-24
**Severity:** Medium (BadLogic) — non-atomic check+write during preload could overwrite a concurrent EP-sync update with a stale snapshot

## Conclusion

`warm_cell` (`src/core/gateway/backends/preload.rs`) now seeds the cell through
`cell.hydrate_if_empty(|| get_roundrobin_store().get_backends_for_service(service_key))`
instead of the old unguarded `if cell.backends.load().is_empty() { cell.replace_backends(backends) }`.
The empty-check and the store now run under the cell's `build_lock` — the **same** lock held by
`replace_backends` (`compiled_selector.rs`) — and the provider re-reads the store inside that
lock, so the freshest backend list is used and no concurrent writer can interleave between the
check and the write. The `backends.is_empty()` pre-check (lines ~144-150) is retained as a cheap
guard so services with no EP data are skipped without even creating a cell.

Re-reports of the form "`warm_cell` has a TOCTOU between `cell.backends.load().is_empty()` and
`replace_backends`", "a Tokio EP-sync watch task can call `replace_backends` with fresher data
between the check and the write", or "preload overwrites a fresher backend list with the stale
snapshot taken at the top of `warm_cell`" are **not accepted** — verify the current `warm_cell`
calls `hydrate_if_empty` before re-reporting.

## What was wrong (before the fix)

`warm_cell` read the backend list once at the top (`get_backends_for_service`), then did a bare
`cell.backends.load().is_empty()` check followed by `cell.replace_backends(snapshot)` with no
lock held across the two steps. The Tokio runtime (started before `preload_load_balancers` runs)
has live config-sync watch tasks; an EP-sync event arriving between the check and the write calls
`update_affected_lbs` (`discovery/conf_handler_common.rs`) → `replace_backends(fresher)`.
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
(`selector.rs::hydrate_cell_if_empty`, used by `select_roundrobin_backend_key` and
`get_backends_for_service_key`). Same TOCTOU-on-a-config-sync-driven-cell defect class as
[backends-F004](set-service-config-toctou-race.md) and
[backends-F002](upsert-task-concurrent-handle-leak.md), fixed with the same
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
- `selector.rs::hydrate_cell_if_empty` — the data-plane caller of the same `hydrate_if_empty` method.
- `discovery/conf_handler_common.rs::update_affected_lbs` — the concurrent `replace_backends` writer.
