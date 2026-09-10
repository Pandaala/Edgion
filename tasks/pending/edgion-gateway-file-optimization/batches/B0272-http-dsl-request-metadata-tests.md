# B0272 — HTTP DSL Request Metadata Tests

## Scope

- Pipeline state: terminal; changed, independently reviewed and verified.
- Active anchor: `edgion-gateway`; baseline refreshed to `a724f56c`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/req_metadata_tests.rs`.
- Read scope may follow metadata builtin dispatch, session abstractions and related tests. Existing integration tests and user-owned handoff/WAF paths remain protected.

## Analysis

- Analyst `/root/b0272_metadata_analysis`: change, simple/test-only; target blob `8b7895642e14027ff484f44872391fcb4886028a`. B0271 changed only Redis tests/records, leaving metadata call chains unchanged.
- Nine existing tests cover meaningful happy/nil paths, but none locks metadata string caps. Cover exact byte limit and one-byte-below limit for protocol, gRPC service/method, certificate CN/fingerprint; observe exact successful values and typed errors, using one multibyte CN and exact-once getters.
- Add gRPC method Some/service None, present-certificate absent-CN, missing-fingerprint Nil, and malformed/overflow/zero Content-Length behavior. Preserve parsing semantics rather than introduce framing validation.
- Exact technical allow-list: target only; existing test helpers suffice. Preserve original tests and leave metadata allocation investigation separate; no new integration gap.
- Original knowledge: DSL engine/security, metadata API/error semantics, byte-vs-char precedent, review routing, unit-testing rules and applicable red design rules; compiler/runtime/session/context call chain traced.

## Implementation and Independent Review

- Implementer `/root/b0272_implementation` added six tests covering the five-getter boundary matrix and missing/parse branches; all nine existing tests remain unchanged. Only the target file changed.
- Fresh reviewer `/root/b0272_independent_review`: PASS, no Must Fix/Should Fix/deferred issue. Verified exact-once getters, byte-based caps, typed errors/equality, observable successful execution and applicable red rules.

## Verification

- Complete Gateway DSL including metadata target: 383/383 passed (38-second build, 0.22-second test execution).
- Format/check, locked workspace check/Clippy, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and integration-freeze checks passed. Clippy exited zero with 982 Gateway lib-test warnings.

## Handoff

- Ledger/changelog updated. Commit pending automatic local commit; never push.
