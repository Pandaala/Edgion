# B0209 — TLS Route Manager

## Scope

- Pipeline state: analysis-ready; third queued candidate, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/routes_mgr.rs`.
- Baseline: `62e43e2f`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0209 analyst.
- Investigate cache and listener-table publication, lifecycle and retained unit
  ownership after B0203/B0204 with direct callers and tests.
- Existing manager documentation task and D0114 are separate; do not duplicate.
- No implementation authorized; integration files and user-owned paths remain protected.

## Analysis Outcome

- Decision: examined-no-change. Target blob `a720fdaca2d5dd7b5c99c4026fa98e9f6fbb9127` at `62e43e2f`; no tests run during analysis.
- Complete tables publish through one ArcSwap store. Cache lookups return owned unit Arcs; listener-manager creation is configuration/startup work, not per-connection global map mutation.
- Full, partial and Gateway-driven rebuilds share the same mutex; the affected-manager helper's sole production caller already holds it. Adding a nested lock would deadlock rather than strengthen serialization.
- Shared partial preparation captures old listeners before mutation and merges new attachments. Full stale sweep, partial orphan sweep and active-empty clear publish empty tables through retained manager handles.
- Rebuilds preserve unit/WRR identity; stats are existing independent size counters, not a business transaction snapshot. Configuration-time vector-copy removal does not justify shared API expansion.
- Existing `route-architecture-manager-cache-documentation.md` owns stale raw-schema/full-rebuild comments; D0114 owns cross-layer hostname projection. No duplicate issue or new technical allow-list.
- Promotion verification: existing TLS manager tests (four statically identified), all TLS tests and TCP manager retained-handle regressions; independent no-change review required.
