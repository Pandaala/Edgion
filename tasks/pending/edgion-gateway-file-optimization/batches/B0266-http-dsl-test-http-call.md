# B0266 — HTTP DSL Outbound-call Tests

## Scope

- Pipeline state: complete; target-only implementation, independent final review, and coordinator gates passed.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `0c547725`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs`.
- Target blob: `83f3791ef2d06f256a3994b22c337076c3d6c730`.
- Read scope includes the complete test file, DSL `http.call` validation/runtime/client/egress/TLS/body-cap paths, response-stage bridging, adjacent tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only unit coverage.
- Add a pre-dial opaque `req.body` regression with five captured bytes, configured DSL cap 1,024, and resolved physical request-body cap four. Assert exact `RuntimeError::BodyAccess` text naming the four-byte effective limit.
- This locks `min(httpMaxRequestBodyBytes, physical request-body cap)`, failure before dialing, fixed fail-close classification, and reuse of the canonical request-body storage without a second cache.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs` only. Preserve clients, EgressResolver, redirects, TLS, response cap, error maps, response-stage bridge, logging, and frozen integration tests.
- D0139 separately owns a new response-stage async timeout-clamp integration scenario; no existing integration file may change in this batch.

## Implementation and Review

- Added the five-byte opaque body versus four-byte physical cap regression with the configured DSL cap intentionally higher at 1,024 bytes.
- Independent final review: LGTM with no findings. It independently confirmed the effective `min(1,024, 4)` bound, pre-send failure, exact typed error, synthetic reader, no network, no second cache, and target-only scope.

## Verification

- Implementer: target 37/37 and full Gateway DSL 335/335 passed; Gateway all-target check, format, diff, English-only, and Gateway no-tracing checks passed.
- Reviewer: target 37/37 and full Gateway DSL 335/335 passed; format, diff, and frozen-path checks passed.
- Coordinator full workspace checks passed: format, locked workspace all-target check and Clippy, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration guards.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `83a82fc1` (`Test DSL outbound physical body cap`); no push.
