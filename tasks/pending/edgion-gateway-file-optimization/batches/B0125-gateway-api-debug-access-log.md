# B0125 — Gateway API Debug Access Log

## Scope

- Baseline: `8a0f44c3e962f70615e75abde4739e4dae9ed527`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/debug_access_log.rs`
- Baseline blob: `d0d3f9719c8e217d1585611847d53da446dad033`
- Proposed exact implementation allow-list:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/debug_access_log.rs`
  - `edgion-resources/src/sensitive.rs`
  - `skills/03-coding/observability/00-access-log.md`
  - `docs/en/user-guide/http-route/filters/edgion-plugins/debug-access-log.md`
  - `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/debug-access-log.md`
  - `docs/en/ops-guide/observability/access-log.md`
  - `docs/zh-CN/ops-guide/observability/access-log.md`
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete debug filter, its route compilation/activation controls, context/diagnostic writes, canonical access-log consumption, tests, and relevant observability review knowledge.
- Request and traffic observability must remain owned by the canonical access log; preserve sensitive-data masking, bounded cardinality/size, operator-only activation, immutable publication, and zero parallel tracing/log mechanisms.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change. `SensitiveMode::Wrap` deliberately preserves raw values inside markers for a trusted local log pipeline, but this filter serializes that representation directly into a client-visible response header. Suppress `X-Debug-Access-Log` in Wrap mode before constructing or serializing the snapshot, add one bounded static plugin diagnostic, and preserve Replace masking plus the explicit Off-mode opt-out.
- Contract correction: the header is a non-terminal response-stage snapshot, not the full terminal access log. It intentionally lacks terminal status/body/timing, this and later response-stage logs, `waf`, and operator-defined `ext` values. All four user/operations pages and the canonical access-log guidance require synchronized wording.
- Complexity: complex. The change crosses crates, changes client-visible sensitive-data behavior and diagnostics on a hot path, and updates bilingual operator contracts. Plan-first and an independent plan review are mandatory.
- Proposed verification: canonical mode-predicate tests plus filter tests for early Wrap suppression/static diagnostic and parseable Replace/Off snapshots, using the existing serialized sensitivity-mode test guard. Existing integration files remain frozen.
- Deferred candidates: D0084 must add separate future wire coverage for Replace, Wrap, hot reload, and startup-global sensitivity configuration; D0085 must define a bounded/protocol-aware response surface and settle the plugin's long-term exposure and non-terminal schema. Duplicate search found no owning task. Create the campaign and canonical records only after promotion review confirms them.
- Freshness: current at `5a95df60`; all seven proposed implementation/support blobs match the original analysis at `8a0f44c3`.
- Review: pending.
- Verification: pending.
- Commit: pending.
