# B0125 — Gateway API Debug Access Log

## Scope

- Baseline: `8a0f44c3e962f70615e75abde4739e4dae9ed527`
- Pipeline state: committed
- Active anchor crate: `edgion-gateway`
- Candidate owned file:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/debug_access_log.rs`
- Baseline blob: `d0d3f9719c8e217d1585611847d53da446dad033`
- Exact implementation allow-list:
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
- Deferred candidates: D0087 must add separate future wire coverage for Replace, Wrap, hot reload, and startup-global sensitivity configuration; D0088 must define a bounded/protocol-aware response surface and settle the plugin's long-term exposure and non-terminal schema. Duplicate search found no owning task. Create the campaign and canonical records only after promotion review confirms them.
- Freshness: current at `5a95df60`; all seven proposed implementation/support blobs match the original analysis at `8a0f44c3`.
- Promotion freshness: revalidated at `5db12c3f`; all seven implementation/support blobs remain unchanged, and the B0124 CORS representation change does not touch this filter, sensitive-mode API, access-log construction, response-stage dispatch, or documentation contract.
- Test isolation: Gateway tests must not mutate the process-global sensitive atomic because their lock cannot synchronize with the resources crate. Test `is_wrap` under the resources module's existing `MODE_LOCK`, and drive Gateway suppression/normal wiring through a private boolean policy seam.
- Deferred ownership: [`D0087`](../deferred/D0087-debug-access-log-wire-integration-coverage.md) owns separately authorized future wire coverage for Replace/Wrap, resource hot reload, and startup-global mode isolation. [`D0088`](../deferred/D0088-bounded-debug-access-log-response-surface.md) owns the protocol-aware serialized-size cap, dedicated non-terminal schema, and invalid/oversize/header-set failure policy while reusing D0068. Their coordinator-owned records require final fidelity review.
- Plan: add the canonical Acquire-load `is_wrap` predicate, suppress the debug header before context access in Wrap mode with the bounded non-error decision `wrap mode:header suppressed`, keep Replace/Off emission unchanged, test global mode only under the resources lock, test Gateway policy through a private boolean seam, and correct all five documentation contracts.
- Independent plan review: the first pass required exact wording that top-level `request_info.status` is always zero at this stage and corrected suppression from `push_err` to the ordinary decision `PluginLog::push`; it also distinguished already-observed upstream-attempt status and unmarked Off values. The revised plan received LGTM at unchanged HEAD `5db12c3f`.
- Implementation: added the canonical Acquire-load `is_wrap`, suppressed the client-visible header before context access in Wrap mode with the bounded ordinary decision `wrap mode:header suppressed`, preserved Replace/Off emission and header-set behavior, added race-free resources and Gateway policy-seam tests, and synchronized the canonical plus four EN/ZH user/operations contracts.
- Independent final review: LGTM with no Must Fix or Should Fix. The reviewer independently confirmed mode publication, test isolation, the zero-work Wrap branch, unchanged non-Wrap behavior, terminal diagnostic attachment, sensitive-data containment, all five documentation contracts, exact seven-file scope, and 14 focused tests.
- Verification: 12 resources sensitivity and 2 Gateway debug-filter tests passed. `cargo fmt --all`, `cargo check --locked --workspace --all-targets`, `cargo clippy --locked --workspace --all-targets`, `cargo build --locked -p edgion-gateway --bin edgion-gateway`, agent-doc, SSA force, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen 3,084-file integration guards passed; Clippy emitted only the existing warning set.
- Commit: `da256709`.
