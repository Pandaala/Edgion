# B0204 — TLS Listener Tables

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/gateway_tls_routes.rs`.
- Baseline: `9cb9bf86`.
- Analyst: `b0204_tls_listener_routes_analysis`.
- Investigate listener-keyed table ownership, matching, publication, clear and
  retained handles, hot reload and stats against current shared boundaries.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.

## Analysis Outcome

- Decision: change; complex/plan-first for winner-only published storage. Target blob `b4d9ec02b57d20175536baaf0364b5e024d149ae`; target baseline 4/4 passed.
- Borrow hostname slices instead of deep-copying Vec/String inputs before lowercasing. Retain temporary candidate vectors and collision ordering/warnings during construction, then publish only the selected Arc per hostname/catch-all; all runtime consumers use first(), and stats do not enumerate candidate vectors.
- Exact proposed scope: target; `skills/04-review/tls/regression-guards/cross-gateway-sni-collision-deterministic.md` for this matcher's storage only; `skills/01-architecture/02-gateway/03-routes/04-tls-route.md` for table/from_units/effective-hostname details.
- Preserve exact > longest wildcard > catch-all, case/multi-depth wildcard behavior, timestamp/name precedence, raw/effective hostname semantics, unchanged unit sharing, collision warnings, and old snapshot validity. Test runner-up promotion after winner deletion and Weak evidence that published tables no longer retain losers.
- Existing tests stay; run all TLS route tests. Independent plan approval is required before code changes.
- High-severity attachment-scoping defect is separately tracked in D0114. Analyst confirms safe continuation with equivalent matching/storage optimization, without claiming to fix listener isolation.
