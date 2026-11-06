# ctrl-center-07: `EventStore.last_version_by_key` retains every key ever deleted for the life of the serving epoch

- **Category**: design-error
- **Severity**: P3
- **Scope**: local
- **Location**: `edgion-controller/src/conf_sync/cache_server/store.rs:37,55-66,68-130`

## Summary

`apply_event` records `last_version_by_key[key] = sync_version` for Add, Update **and Delete**, and the
map is only cleared by `clear()` (serving-epoch teardown). Keys of deleted resources are never
removed, so the map grows with the number of distinct resource names ever seen, not with live
resources. The guard it powers is also unreachable in current code.

## Current behavior (evidence)

- `store.rs:115`: `self.last_version_by_key.insert(key.clone(), sync_version)` on every event type;
  `data.remove(&key)` on Delete leaves the version entry behind.
- The only callers pass `sv = None` (`processor.rs:917,988`), so versions are drawn from the global
  counter inside the same write lock (`cache.rs:252-266`) and are strictly increasing per store; the
  per-key "stale event dropped" branch cannot trigger.
- High-cardinality name churn is normal: EndpointSlices use generated names and are created/deleted
  as Services scale; preview/CI namespaces create and delete whole sets of Services, Slices, Secrets,
  ConfigMaps and routes. A long-running leader accumulates all of them.

## Impact

Slow, unbounded memory growth in the controller proportional to historical name churn; no functional
benefit.

## Suggested direction

Remove the entry on Delete (or drop the map entirely and keep the store-wide monotonic check, since
versions are allocated under the lock). If the per-key guard is kept for future pre-assigned versions,
bound it to live keys.

## Notes

The ring-buffer intentional-design page covers event retention, not this side map. The cache-residue
fix referenced in `processor.rs:970-975` moved version allocation under the lock, which is what made
the per-key map redundant. No `tasks/` entry.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P3
- `edgion-controller/src/conf_sync/cache_server/store.rs` (`apply_event`): `last_version_by_key.insert(key, sync_version)` for every event type; Delete only does `data.remove(&key)`; map cleared only in `clear()`.
- `edgion-controller/src/conf_sync/cache_server/cache.rs:262-265`: version is drawn via `sv.unwrap_or_else(next_resource_version)` under the same write lock, and processor callers pass `None` (`processor.rs:914` and siblings), so the per-key stale guard is redundant.
- Small per-entry cost; growth proportional to name churn (EndpointSlices) over a long leader term — P3 correct.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3 (unchanged)
- `edgion-controller/src/conf_sync/cache_server/store.rs:37,65,79,115`: `last_version_by_key` still inserted for every event type, cleared only in `clear()`; file unchanged since `feature-08-30`.
- `processor.rs` unchanged, so callers still pass `sv = None` and the per-key guard stays redundant.
