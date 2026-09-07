---
knowledge_type: regression-guards
---

# AI runtime stores must report cache residue

Process-global AI credential-outlier and adaptive-telemetry maps have
independent resource lifecycles. They must expose `count()` and snapshot
`keys()` methods, and gateway `StoreStatsResponse` must include each count so
the final `cache_residue` pass can detect missed AI-service or route cleanup.

Tests must distinguish an empty store from a hard-coded zero by checking the
empty, populated, and removed states. The production store-stats collector is
also exercised with populated local stores and must report both counts as one;
the serialization test requires both response fields. The shared integration
topology already loads AI services and routes and ends with wipe-all plus
cache-residue validation; do not add a second topology solely for these stores.

## Resolution references

- `gw-05` — 2026-08-09.
