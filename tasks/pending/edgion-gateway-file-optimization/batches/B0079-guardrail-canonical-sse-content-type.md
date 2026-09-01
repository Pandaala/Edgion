# B0079 — Guardrail Canonical SSE Content-Type Classification

## Scope
- Baseline: `7f69df81`
- Pipeline state: committed and fully verified
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/guardrail/{attached.rs,mod.rs,plugin.rs,protocol.rs,response.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/guardrail/plugin.rs`
- Necessary supporting file: `edgion-gateway/src/routes/http/proxy_http/content_type.rs`; this support edit does not claim campaign coverage for the routes module
- Expected no-change files: `attached.rs`, `mod.rs`, `protocol.rs`, and `response.rs`
- Allowed tests: colocated tests in the implementation and supporting files only
- Forbidden files: every existing `edgion-tests/**` path, RawWindow and response-state machinery, resources/schema, shared outbound implementation, access-log and metrics implementation, documentation, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the implementation and supporting files; campaign ledger/changelog/main record; B0078 and B0079 batch records

## Context and value gate
- Analysis subagent: `b0078_geoip_analysis`, revalidated against `7f69df81bbc84b09874978dd850bf1a1c286cd07`; target blobs are `6bfa4360b19d83e304197de844ce7bb8621ee2dd` for `attached.rs`, `68f5a99aeae9331c5925a1fcab46cd352f43e5d5` for `mod.rs`, `383dea8d7acc65b42f18a57208f590b794b7bd94` for `plugin.rs`, `055014f2ad703c93dd23456438e66c4240cfc115` for `protocol.rs`, and `8b8522aca4e1da131e2987bd89a8589a18882389` for `response.rs`.
- Baseline focused tests: 20 Gateway Guardrail and 7 resource Guardrail tests passed.
- Guardrail independently parsed `Content-Type` with split, trim, and case folding even though the proxy response path already owns the canonical exact SSE media-type predicate.
- The local chain also materialized a second temporary String from the response header on every eligible response.
- Decision: expose the existing predicate at crate visibility, keep proxy classification on the same predicate, and borrow the Guardrail response header through that canonical implementation.
- Expected no-change files retain coherent request attachment, bounded protocol parsing, RawWindow generation ownership, access-log summary, response-head hold, cancellation, and terminal arbitration.

## Hard-rule boundary
- Preserve the 2xx status gate, identity content encoding, canonical AI processor source, first-head Hold behavior, policy Reject precedence, and fail-open/fail-close semantics.
- Preserve main upstream streaming, RawWindow ownership, response chunk backpressure, rollback behavior, bounded decision outbound exchange, and request BodyStorage boundaries.
- Add no tracing, metrics, access-log fields, log detail, outbound/cache mechanism, protocol/config/schema behavior, dependencies, or integration-test edits.
- The helper matches only the exact SSE media type, case-insensitively, with optional whitespace and parameters; longer subtype extensions remain rejected.

## Deferred candidates
- `tasks/todo/sse-window-strict-residency-and-continuity.md` already owns rollback allocation, replacement preview copying, metadata residency, overlap continuity, and EOS preview as one design cluster. This batch does not partially repair or duplicate that task.
- Existing frozen integration coverage includes pass, replacement, rejection, first-head rejection, malformed fail-open, timeout fail-close, ExtProc composition, and Anthropic cases. No new integration-test issue is required.

## Implementation
- Complexity: local; no plan-first review required
- Implementer: `b0055_runtime_analysis`, distinct from the analyst and final reviewer
- Exact change: added the crate-visible `is_sse_content_type` wrapper around the canonical matcher; proxy stream classification continues through it; Guardrail now borrows its response header and calls the same predicate without the temporary owned String
- Test contract: canonical direct and response-header classification accept case variants and parameters while rejecting empty, unrelated, and longer subtype values

## Independent review
- Code reviewer: `b0079_final_review`, distinct from the analyst and implementer
- Code verdict: approved with no correction; confirmed minimal visibility, exact media-type semantics, allocation removal, behavior preservation, support-file classification, and deferred-task ownership
- Deferred adjudication: existing strict-residency/continuity task only; no new deferred or integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | protected external paths unchanged |
| Baseline focused tests | passed | 20 Gateway and 7 resource Guardrail tests |
| Implementation and focused tests | passed | 20 Guardrail, 6 canonical content-type, and 7 resource tests |
| Gateway all-target compile | passed | implementation run passed |
| Independent code review | passed | reviewer reran focused tests, format, no-tracing, diff, and frozen checks |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 4 examined-no-change owned rows; supporting route file is not claimed
- Commit: `e6034b85`
