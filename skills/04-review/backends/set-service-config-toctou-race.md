---
name: set-service-config-toctou-race
description: CircuitBreakerStore::set_service_config now does a single DashMap entry() check-and-swap under one shard lock. Reject re-reports claiming the get/remove/insert sequence has a TOCTOU lost-update race or a transient breaker-bypass window.
---

# backends-F004 — `set_service_config` config update is now an atomic `entry()` swap

**Status:** ✅ fixed · 2026-05-21
**Severity:** Medium (BadLogic) — non-atomic config update could lose a concurrent update and briefly bypass the breaker

## Conclusion

`CircuitBreakerStore::set_service_config` (`edgion-gateway/src/backends/circuit_breaker/store.rs`)
now performs the "config changed → rebuild state" path through a single
`self.services.entry(service_key.to_string())` match, so the check-and-swap runs under one
DashMap shard write lock:

- `Entry::Occupied` + config differs → `occ.insert(new_state)` swaps in the fresh
  `ServiceCbState` and returns the displaced `Arc`; the entry guard is dropped **before**
  `drain_ejected_gauge` runs (the drain only touches the old state's own `endpoints` sub-map
  and the metrics layer, never `self.services`, so there is no re-entrant shard deadlock).
- `Entry::Occupied` + config unchanged → no-op (existing endpoints / ejection state preserved).
- `Entry::Vacant` → insert the new state.
- `None` (clear) → unchanged: `remove` + `drain_ejected_gauge`.

Re-reports of the form "`set_service_config` has a TOCTOU between `get` and `remove+insert`",
"two concurrent updates for the same key lose an update / the last write is not guaranteed to
win", or "the `remove`→`insert` gap lets data-plane readers (`is_ejected` / `record_failure`)
momentarily bypass the breaker" are **not accepted** — verify the current `set_service_config`
uses the `entry()` check-and-swap before re-reporting.

## What was wrong (before the fix)

The `Some(cfg)` branch ran four independent DashMap operations: `get` (read shard lock) →
`drop(existing)` (release) → `remove` → `insert`. Two problems:

1. **Lost update**: between `drop` and `remove`, a concurrent `set_service_config` for the same
   key could read the same old config, compute its own diff, and also `remove+insert`; the
   later `insert` overwrote the earlier one with no "most-recent-wins" guarantee, silently
   (no panic, no log).
2. **Transient breaker bypass**: even single-threaded, the window between `remove` and `insert`
   left the key absent. Data-plane readers (`is_ejected`, `record_failure`) run concurrently on
   the hot path and key off `self.services.get(service_key)`; during that window they observed
   `None` and skipped circuit-breaker logic for that service.

The race was reachable because circuit-breaker config is driven from the Service config-sync
handler (`discovery/services/conf_handler_impl.rs` `full_set` / `partial_update`), and a rapid
`full_set` + `partial_update` overlap for the same key could interleave two writers.

## Why the fix is correct

A single `entry()` holds the shard write lock across the read of the existing config and the
swap, so no other writer can interleave between the check and the insert — the writer holding
the lock is the linearization point and the last one wins deterministically. Because the entry
is only ever swapped (never removed-then-reinserted), there is no instant where the key is
absent, so concurrent data-plane readers always see either the old or the new state, never
`None`. The displaced `Arc` is returned by `insert`; dropping the guard before draining its
gauge keeps the critical section minimal while preserving the `CB_EJECTED_ENDPOINTS` +1/-1
balance. This is the same defect class as [backends-F002](upsert-task-concurrent-handle-leak.md)
(non-atomic multi-step mutation on a config-sync–driven map), fixed with the same
atomic-swap principle.

## Fix Suggestions Not Accepted

- "Add a per-service `Mutex` to serialize `set_service_config` per key" — heavier than needed;
  the single `entry()` shard lock already makes the swap atomic with no extra keyed state.
- "Derive `PartialEq` on `CircuitBreakerConfig` and compare with `!=`" — out of scope; the
  explicit three-field comparison is preserved unchanged to keep the diff minimal.

## Re-evaluation Triggers

Re-open only if:

- `set_service_config` reverts to a `get`/`remove`/`insert` (or any multi-operation) form that
  reopens the absent-key window.
- `drain_ejected_gauge` gains a path that touches `self.services` (which would make draining
  under the held entry guard a re-entrant deadlock — the fix relies on it not doing so).

## Reference Cases

- backends-F004 (review finding, 2026-05-21)
- backends-F002 — sibling fix, same atomic-swap principle on the health-check task map.
- `discovery/services/conf_handler_impl.rs` — the Service handler `full_set` / `partial_update`
  that drive `cb_store.set_service_config`, the source of the same-key writer overlap.
