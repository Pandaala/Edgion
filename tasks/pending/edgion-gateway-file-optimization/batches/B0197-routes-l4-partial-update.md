# B0197 — L4 Partial Update

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/l4_partial_update.rs`.
- Baseline: `56cda803`.
- Analyst: `b0197_l4_update_analysis`.
- Investigate delta application, listener ownership, atomic publication/removal,
  shared callers, and hot-reload invariants before selecting a value outcome.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.

## Analysis Outcome

- Decision: examined-no-change. Target blob: `abb30d797d54c804e57e4d419fab6426f556d251`.
- All three callers serialize under the manager rebuild lock. The helper collects old remove/update/add listeners before mutation and publishes affected scopes once; new/old listeners remain covered and unaffected Arc units are reused.
- Sync event buckets are mutually exclusive by key. Misclassified-add old-listener handling already has regressions. Initializers currently always succeed, so speculative failed-add changes lack a reachable value case.
- Rejected cold-path allocation cleanup, infallible Result removal across callers, and redundant helper tests as disproportionate or duplicative.
- Fresh caller unit tests: TCP 2/2, TLS 7/7, UDP 7/7. No files modified.
- Unrelated TLSRoute feature guidance still describes version fallback contrary to the atomic-upgrade rule; track separately rather than using it as current behavior evidence.
