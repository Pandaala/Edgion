# B0278 — HTTP DSL Runtime Errors

- Pipeline state: terminal; examined-no-change, no technical mutation.
- Active anchor: `edgion-gateway`; baseline `e8bd209f`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/runtime_errors.rs`.
- Analyst: `b0278_runtime_analysis`; examined-no-change. Ten tests directly pin checked arithmetic operation errors, call/stack bounds, host error ownership and manual underflow; sibling suites own policy/panic behavior. Further permutations, helper extraction and heading-count cleanup rejected as low value.
- Existing integrations and user-owned handoff/WAF paths remain protected.

## Independent review and scope

- `b0278_review`: LGTM, no disagreement or Must Fix; independently verified original compiler/VM/helpers/rules, no-change value and D0145 classification. Target blob `5ecb7ddc354b0702c63f18f88ab1603b0fccce42` unchanged.
- Analyst read original design, DSL engine/security/error contracts, review routing/DSL/VM observability precedent and unit testing. Logging/access-log, redaction, restraint, unification, streaming and English-only remain explicit gates.
- Incidental D0145 records two real-network sandbox unit tests separately. User instruction overrides the analyst's possible immediate integration migration: any needed new end-to-end coverage is another deferred task, not an integration edit now.
- Commit scope: this record, ledger/main/changelog, B0277 SHA backfill, standalone D0145 task and coordinator deferred backlink/index. No technical target change.

## Verification and handoff

- Targeted runtime-error tests 10/10 passed. Fresh locked workspace check/Clippy, applied format/format check, agent docs, SSA, metrics, Pingora source, Gateway tracing boundary, English-only, diff and frozen-path checks passed; existing Clippy warnings remain.
- Ledger/changelog updated. Commit: `1edac7b7`; no push.
