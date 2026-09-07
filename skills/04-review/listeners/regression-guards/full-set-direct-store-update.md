---
name: full-set-direct-store-update
description: Historical review evidence for Gateway handler; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Full set direct store update

## Recorded conclusion

`GatewayHandler::full_set` dead `tracing::warn!` on `add_gateway` Err branch removed; `full_set` now calls `store.update_gateway` directly after `store.clear()`

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `g8-008` — —.
