---
name: consistent-hash-duplicate-node-budget
description: ConsistentHashRing::select now charges the max_iterations budget only for distinct-backend attempts and breaks once all distinct backends are evaluated. Reject re-reports claiming duplicate virtual nodes exhaust the budget before reaching a healthy peer, or that the early-exit is optional.
knowledge_type: regression-guards
---

# lb-F004 — consistent-hash walk charges budget per distinct backend, not per ring slot

**Status:** ✅ fixed · 2026-05-24
**Severity:** Medium (BadLogic) — duplicate virtual nodes of a high-weight unhealthy backend could exhaust the iteration budget before the walk reached a distinct healthy peer

## Conclusion

`ConsistentHashRing::select` (`edgion-gateway/src/lb/selection/consistent_hash.rs`) now
increments the iteration counter (`steps`) **only when `seen.insert(backend_idx)` succeeds**
(i.e. for a newly-encountered distinct backend), not on every ring slot. A duplicate virtual
node belonging to an already-seen backend takes the `continue` branch without charging the
budget. The walk also breaks once `steps >= distinct_backends` (the count of unique Inet
addresses, `self.addr_to_idx.len()`), so all distinct backends are evaluated before giving up.

Re-reports of the form "duplicate virtual nodes consume `max_iterations` prematurely",
"a high-weight unhealthy backend makes `select` return `None` despite healthy peers", or
"the `steps += 1` is charged before the duplicate-backend check" are **not accepted** —
verify the current `select` charges `steps` inside the successful-insert branch before
re-reporting.

## What was wrong (before the fix)

`steps += 1` ran immediately after `ring.get_addr()`, before the duplicate-backend guard.
`pingora_ketama::Continuum` places `DEFAULT_POINT_MULTIPLE = 160` virtual nodes per weight
unit, so a backend with weight W occupies 160×W ring slots. With `SELECT_MAX_ITERATIONS = 256`
(`compiled_selector.rs`), a single unhealthy backend with weight ≥ 2 whose vnodes cluster near
the hash key could consume the entire budget on its own duplicate slots — the loop exited at
`steps >= max_iterations` and returned `None` even though healthy peers existed elsewhere on
the ring. Operationally this turned a partial upstream failure into a 502/503 instead of a
failover (silent availability degradation).

## Why the fix is correct

Counting only distinct-backend attempts means the budget measures real health-check progress:
each unit of budget corresponds to one backend actually evaluated by `health_filter`, so the
walk reaches every distinct backend within `min(max_iterations, distinct_backends)` charged
steps regardless of how the duplicate vnodes are distributed.

The early-exit (`steps >= distinct_backends`) is **mandatory, not optional**:
`Continuum::get_addr` never returns `None` on a non-empty ring (it wraps around forever,
`pingora-ketama-0.8.0/src/lib.rs:394-401`). Once every backend has been seen-but-rejected,
`seen.insert` always returns `false`, `steps` freezes, and without the break the loop would
spin over duplicate vnodes forever. Because each successful insert charges exactly one step,
`steps` equals the number of distinct backends seen, so `steps >= distinct_backends` is the
"all distinct backends evaluated" condition.

The bound is keyed off `self.addr_to_idx.len()` (distinct Inet addresses), **not**
`self.backends.len()`: non-Inet backends (e.g. Unix sockets) are excluded from the ring and
from `addr_to_idx`, so they can never enter `seen`; using `backends.len()` would make the
break condition unreachable when such backends are present and reintroduce the infinite loop.

## Fix Suggestions Not Accepted

- "Keep `steps += 1` on every ring slot and only add the early-exit" — does not fix the bug;
  the duplicate vnodes still consume the budget before all distinct backends are reached.
- "Use `self.backends.len()` for the early-exit (as the original issue suggested)" — not
  termination-safe when non-Inet backends are present; use `self.addr_to_idx.len()`.
- "Drop the `steps`/`max_iterations` cap entirely now that we break on distinct backends" —
  the cap is retained as a secondary upper bound for fleets larger than `SELECT_MAX_ITERATIONS`.

## Re-evaluation Triggers

Re-open only if:

- `select` reverts to charging `steps` before the duplicate-backend guard.
- The early-exit is removed or rekeyed off `self.backends.len()`.
- A future `pingora_ketama` upgrade changes `get_addr` to terminate the walk on its own
  (which would relax — but not invalidate — the mandatory-break reasoning).

## Reference Cases

- lb-F004 (review finding, 2026-05-24)
- Unit regressions in `consistent_hash.rs`:
  `high_weight_unhealthy_backend_does_not_starve_healthy_peers` (budget = distinct count still
  finds a healthy peer) and `all_unhealthy_with_heavy_backend_terminates` (large budget,
  all-unhealthy heavy backend; proves the distinct-backend break terminates the walk).
- `compiled_selector.rs` — `SELECT_MAX_ITERATIONS = 256`, the only non-test caller of `select`.

## Resolution references

- `lb-F004` — 2026-05-24.
