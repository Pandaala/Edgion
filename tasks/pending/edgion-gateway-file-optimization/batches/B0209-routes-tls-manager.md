# B0209 — TLS Route Manager

## Scope

- Pipeline state: verified examined-no-change; awaiting local commit.
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

## Promotion and Verification

- Original analyst revalidated at `f48fca38`; target and configuration/shared-helper dependencies unchanged. Both proxy helpers preserve manager publication and selected-generation contracts while shortening read guards.
- Independent reviewer: `b0209_manager_review`; Approved at `f48fca38`, target blob unchanged. Personally verified configuration/Gateway lock ownership, pre-mutation listener union, complete publication, retained-handle clear/sweeps and actual proxy consumption. Fresh independent TLS manager 4/4, all TLS 54/54 and TCP manager 5/5 passed. No production or integration edits.
- Fresh coordinator TLS manager 4/4, TCP manager 5/5 and all TLS 54/54 passed, along with workspace format/check/Clippy, docs (1,264 files), SSA, metrics, Pingora source, Gateway tracing, English-only and 3,120-file freeze. Commit pending; no push.
