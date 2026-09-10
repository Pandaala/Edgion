# B0279 — HTTP DSL Sandbox

- Pipeline state: terminal; changed.
- Active anchor: `edgion-gateway`; baseline `1edac7b7`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/sandbox.rs`.
- Analyst: `b0279_sandbox_analysis`; change. Full sandbox/helper/VM limit dispatch, policy, HTTP completion and related unit/integration coverage examined.
- D0145 already owns the two network-unit-test isolation cases. Existing integration and user handoff/WAF paths remain protected.

## Value and scope

- Loop test's generic 500 also passes when only total step budget fires, masking a missing loop limit. Add precise LoopLimitExceeded limit 3, StringTooLong len 11/limit 5 and MapTooLarge size 3/limit 2 to three existing tests while preserving their Deny assertions.
- Reuse existing raw-error helper and cloned limits; keep all eleven tests, default compile versus stricter runtime Map limits, and D0145 cases unchanged. Adjacent Split errors already have exact coverage; no helper/style refactor.
- Target is sole technical write path. Commit also owns this record, ledger/main/changelog and B0278 SHA backfill.
- Original design, DSL engine/security/error/http-call, review routing/bounded HTTP response and unit-testing rules read by analyst; later roles independently read originals and check logging/access-log/redaction/restraint/unification/English/streaming gates.
- Verify three changed tests with exact filters and error_policy suite; avoid full sandbox/DSL filters that invoke deferred network unit tests.

## Implementation and review

- Implementer `b0279_impl`: three typed error assertions added, all eleven tests and original Deny checks retained; blob `28292e9eaf3b17860e2fed25654aca325c903c4e`.
- Fresh reviewer `b0279_review`: LGTM, no disagreement/Must Fix/Should Fix. Verified compiler/runtime limit separation and specific error triggers. No production or integration changes.

## Verification and handoff

- Three changed tests each passed with exact filters; error_policy 9/9 passed. Deferred network cases were not invoked.
- Locked workspace check/Clippy, applied format/format check, agent docs, SSA, metrics, Pingora source, Gateway tracing boundary, English-only, diff and frozen-path guards passed. Existing Clippy warnings remain (982 Gateway lib-test warnings).
- Ledger/changelog updated. Commit: pending automatic local commit; never push.
