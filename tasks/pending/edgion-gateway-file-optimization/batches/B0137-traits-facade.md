# B0137 — Plugin Runtime Traits Facade

## Scope

- Baseline: `2fffc7049cb58f8a3831e7a328d57453935b5485`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/runtime/traits/mod.rs`
- Baseline blob: `b1a6723bd02f6ea29607f8bfe56c000b350e26ed`
- Implementation scope: not approved; read-only queued candidate only

## Handoff

- Analysis: `b0137_traits_facade_analysis` read the complete facade, layered runtime/plugins exports, macro-generated mock paths and all consumer classes. Decision: examined-no-change candidate.
- Value verdict: every flat export has real consumers; lower-level session/body/error/test-helper APIs intentionally remain reachable through named submodules; module visibility supports macros and cross-module tests. Pruning or path migration would create broad API churn without behavioral, allocation or ownership benefit.
- Preserved contracts: the single resources-owned `DyeOutcome`, internal-only streaming processor seam, test-only helper exposure, plugin-author facade, body and observability boundaries.
- Review: pending promotion and independent no-change review.
- Verification: pending.
- Commit: pending local record commit; never push.
