# B0208 — TLS Route Unit

## Scope

- Pipeline state: verified examined-no-change; awaiting local commit.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/route_unit.rs`.
- Baseline: `62e43e2f`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0208 analyst.
- Investigate immutable schema generations, selector ownership, predicates and
  plugin lookup with actual manager/table/proxy callers.
- Existing integration files and user-owned paths are protected; no implementation authorized.

## Analysis Outcome

- Decision: examined-no-change. Target blob `13dd7e50c9afb0467d0c3264b343a222f54ca581` at `62e43e2f`; no tests run during analysis.
- Incoming schema Arc moves into the unit. Configuration-time backend cloning gives the canonical selector independent owned data; initialization completes before publication, while empty rules fail with the existing no-backend result.
- Handler changes create new units; listener rebuilds and multiple hostnames reuse units and selector state. Removing the selector's outer Arc offers only a small configuration-time allocation saving and changes independent identity tests without sufficient value.
- Retry bounds/defaults, keepalive and stream keys use shared annotation resolvers. Plugin keys remain live lookup inputs rather than stale runtime snapshots.
- Preserve denied/unsupported declared weights and immediate post-select termination. Mixed optional weights follow the shared all-or-none selector policy; no TLS-only normalization or conformance claim.
- No additional defect or integration gap; D0114/D0115/D0116 remain separate. At promotion, independently review and run target unit tests (nine statically identified), shared backend selector and all TLS tests.

## Promotion and Verification

- Analyst revalidated at `ac3dfe7c`: target blob, selector/resolver/handler/table unchanged; both new proxy helpers preserve the owned unit API and shared state. Original new ownership knowledge was read directly.
- Independent reviewer: `b0208_unit_review`; Approved, no target-local Must Fix or Should Fix. Personally verified original rules, full target and real configuration/selection/plugin consumers at `ac3dfe7c`, target blob unchanged. Fresh independent TLS 54/54 and shared selector 22/22 passed. No production or integration changes.
- Fresh coordinator unit 9/9, shared selector 22/22, TLS 54/54, workspace format/check/Clippy, docs (1,264 files), SSA, metrics, Pingora source, Gateway tracing, English-only and 3,120-file freeze passed. Commit pending; no push.
