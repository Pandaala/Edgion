# B0038 — Plugin Outbound Request

## Scope

- Baseline: `4a576009545a1d486130f234366bd3b5139c6721`
- Pipeline state: `complete`
- Active anchor crate: `edgion-gateway`
- Anchor file: `edgion-gateway/src/plugins/http/common/outbound/request.rs`
- Approved shared support: `edgion-resources/src/resources/common/outbound_request.rs`
- Forbidden files: every other source or test file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Pre-existing protected paths: `task/optimization-by-files.md`, `tasks/pingora-0.9.0/`, `tasks/todo/complete-k8s-dynamic-scenario-parity/`, and `waf-architecture-and-multi-pattern-matching.md`.
- Exact commit allow-list: the two source files plus B0037/B0038 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis subagent: `coverage_plan` in a B0038 analysis turn.
- Decision: `change`, simple. Shared outbound request validation did not check explicit `headers.origin[].name` or `headers.custom[].name`. Gateway runtime parsing silently skipped invalid names, so accepted credentials, auth context, or routing headers could disappear.
- Unification decision: reuse canonical `gwapi_types::is_valid_header_token` and `HEADER_NAME_MAX_LEN`; do not add an HTTP dependency or duplicate regex.
- Defense boundary: shared admission rejects static malformed names, while `CompiledRequest::from_spec` invokes the same validation for direct/future callers. Runtime parsing remains defense in depth for template-produced dynamic values.
- Preserved semantics: args/cookies forward-all precedence, proxy framing rejection and synchronization, explicit Host/Expect contracts, duplicate and sensitive-header behavior, template compilation, secretRef collection, percent/JSON encoding, body replayability, authority pinning, and B0037 detail-free errors.
- Freshness revalidation: valid at `4a5760095`, including the committed outbound error redaction.

## Implementation

- Validated every explicit origin/custom header name for nonempty canonical token syntax and the inclusive 256-byte bound, returning only bounded structural position errors.
- Added the shared validation call at Gateway compilation and retained runtime header parsing.
- Added malformed, empty, control, non-ASCII, oversized, punctuation, exact-boundary, and direct-constructor regressions. Adjusted one presence-policy matrix to exclude already-invalid args/cookies forward-all configurations covered by dedicated validation tests.
- Changed no schema field, serialization, transport, log, metric, client, resolver, or failure policy.

## Independent review

- Reviewer: `b0001_review`, independent from the B0038 analyst and implementer.
- Verdict: approved with no findings after checking canonical validation semantics, both header categories, ordering and non-reflection, exact boundary/token coverage, direct callers, runtime defenses, preserved framing/Expect/Host contracts, test matrix coverage, schema stability, and exact scope.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | Resource 23, Gateway request 16, complete outbound 88, outbound client 30, and ForwardAuth resource 12 tests passed. |
| Independent review checks | passed | Resource/Gateway checks and Clippy, formatting, diff, exact scope, and frozen checks passed. |
| Full repository gates | passed | Workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only pre-existing Kafka/frozen-test lint and duplicate-dependency warnings remain. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths remain byte-for-byte and mode-for-mode unchanged. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `07b2af5c7e032e98ed55101b0993cde6ddca6612`.
