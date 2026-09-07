# B0127 — Gateway API Header Modifiers

## Scope

- Baseline: `da2567094703a53ad928698be10388b450e39e43`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/request_header_modifier.rs`
  - `edgion-gateway/src/plugins/runtime/gateway_api_filters/response_header_modifier.rs`
- Baseline blobs: `ba64470dc380d69dee6fc33cbae853cfcefd5e9f` and `9b741e497b3156830f69cd2ab86fc9d84e4da972`
- Candidate implementation allow-list: not frozen until analysis completes
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read both header-modifier adapters, shared schema validation, compilation/publication, request and response mutation semantics, tests, docs, and relevant review knowledge.
- Preserve ordered add/set/remove behavior, header safety and sensitivity, immutable publication, canonical diagnostics/access-log ownership, observability restraint, and streaming.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change both owned files. Gateway API v1.5 treats case-insensitively equivalent names within each set/add/remove list as first-entry-wins, but the runtime executes every entry, so later set values win and duplicate add values append. Normalize once during immutable construction while preserving the first spelling/value/order and the cross-operation `set → add → remove` sequence.
- Proposed scope: both owned adapters; `gateway_api_filters/mod.rs`; new private `gateway_api_filters/header_modifier.rs`; and paired EN/ZH request-header-modifier docs. The shared plan consumes HTTP/GRPC configs, retains private immutable slices, and performs no per-request folding or allocation.
- Complexity: complex and plan-first. Although normalization is locally simple, it changes client-visible HTTP/GRPC request/response behavior, introduces a shared runtime module, and updates bilingual contracts.
- Preserved behavior: HTTP/GRPC parity, append of pre-existing field lines, cross-list removal precedence, current runtime parse/error diagnostics pending shared validation, response streaming, no sensitive configured values in logs, no tracing/metrics, and no plugin-local framing policy.
- Deferred candidates: complete native/EdgionPlugins header-modifier validation and immutable typed preparation parity; separately unify proxied request-header mutation and framing ownership after confirmed `Content-Length`/body divergence. D0068 remains response-only and must not be duplicated. Reserve D0090–D0091 after B0126's D0089; no integration issue is currently needed.
- Freshness: analyzed at `da256709`; both owned blobs and the module/docs support blobs matched HEAD, while the proposed shared module is absent.
- Review: pending.
- Verification: pending.
- Commit: pending.
