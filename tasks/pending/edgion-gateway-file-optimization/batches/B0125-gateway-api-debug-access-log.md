# B0125 — Gateway API Debug Access Log

## Scope

- Baseline: `8a0f44c3e962f70615e75abde4739e4dae9ed527`
- Pipeline state: analysis-in-progress
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/debug_access_log.rs`
- Baseline blob: `d0d3f9719c8e217d1585611847d53da446dad033`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete debug filter, its route compilation/activation controls, context/diagnostic writes, canonical access-log consumption, tests, and relevant observability review knowledge.
- Request and traffic observability must remain owned by the canonical access log; preserve sensitive-data masking, bounded cardinality/size, operator-only activation, immutable publication, and zero parallel tracing/log mechanisms.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: pending.
- Review: pending.
- Verification: pending.
- Commit: pending.
