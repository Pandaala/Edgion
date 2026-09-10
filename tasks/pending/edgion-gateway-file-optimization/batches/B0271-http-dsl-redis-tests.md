# B0271 — HTTP DSL Redis Builtin Tests

## Scope

- Pipeline state: terminal; changed, independently reviewed and verified.
- Active anchor: `edgion-gateway`; baseline `209cd8c1`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/redis_builtins.rs`.
- Read-only investigation includes Redis dispatch, static validation, LinkSys ownership and related tests; implementation is confined to the target file.
- Existing integration tests and user-owned handoff/WAF paths are protected.

## Analysis

- Analyst `/root/b0271_redis_analysis`: change, simple. Target blob `06f69903fac737161204e679893a21799fadf640`.
- Current rate-limit budget test accepts either MapTooLarge or StringTooLong for both cases, hiding wrong error types/fields. Assert exact map size 5/limit 4 and string length 18/limit 17 via direct VM execution.
- Add an exact simultaneous map/string budget (5/18) success boundary reaching the unregistered provider's complete `ok=false,value=nil,error=provider_not_ready` envelope with observable script completion; no network or global client setup.
- Exact technical allow-list: target only. Preserve all other tests; existing Redis-error redaction and LinkSys-key clone issues remain separate, with no duplicate task.
- Original knowledge: DSL engine/security, Redis builtin/error contracts, review routing/DSL/LinkSys/testing, rate-limit policy/alias heuristic/read truncation precedents and unit-testing rules.

## Implementation and Independent Review

- Implementer `/root/b0271_implementation` completed exact typed budget failures and an observable exact-budget provider-not-ready boundary in the target file only.
- Fresh independent reviewer `/root/b0271_independent_review`: PASS, no Must Fix. Verified direct typed errors, full envelope and strict completion mock, provider lookup without network/global-state mutation, scope and red rules. No interactive issue-repair workflow was used.

## Verification

- Full Gateway DSL including Redis target: 377/377 passed (41-second build, 0.21-second test execution).
- Format/check, locked workspace compile/Clippy, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and frozen integration guards passed. Clippy exited zero with 982 Gateway lib-test warnings.

## Handoff

- Ledger/changelog updated. Commit pending automatic local commit; never push.
