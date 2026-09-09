# B0258 — HTTP DSL Body-value Tests

## Scope

- Pipeline state: in-progress; refreshed analysis passed the value gate after B0257.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `222f59b7`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/body_value.rs`.
- Target blob: `cc8c2f5774ae3adc1384115fd263d603f2d1d3d5`.
- Read scope includes the full test file, every exercised body-value builtin/runtime path, request `BodyStorage`, fixtures, adjacent tests, and current body/DSL review precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only test coverage.
- The compiler rejects only a literal empty `req.body.replace` search. A dynamic empty string compiles to `BodyReplace` and depends on the VM's unique `non-empty Str` runtime guard before scanning or mutation.
- Without that guard, the lower storage `EmptyPattern` error maps to fixed fail-close `BodyAccess`, changing the script-visible error contract from `TypeError` and bypassing configured DSL error policy.
- Add one focused non-literal empty-search test asserting the exact `TypeError` fields. Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/body_value.rs` only.
- Preserve compile-time literal rejection, scan budgets, mutation/storage behavior, error policy, observability, and integrations. D0129 remains the sole owner of body-read pre-allocation bounds.

## Implementation and Review

- Added one non-literal empty-search test asserting the exact `TypeError` contract.
- Zero-call expectations on context access and request-body replacement prove rejection occurs before cap/scan-budget lookup and mutation dispatch.
- Independent final review: LGTM, no Must Fix, Should Fix, or new deferred finding. The compiler, VM, session, storage, error-policy, and D0129 boundaries were independently traced.

## Verification

- Focused body-value tests: 12/12; full DSL tests: 329/329; literal-empty compiler regression passed; Gateway locked all-target compile; format and diff checks passed.
- Coordinator: `cargo fmt --all`; locked workspace check; locked workspace Clippy (exit 0 with pre-existing warnings); agent docs; SSA force; metrics inventory; Pingora source policy; Gateway no-tracing; English-only; diff check; and frozen paths all passed.
- Frozen integration manifest: 3,231 paths unchanged.
- Commit: `9610ead5` (`Test dynamic empty body replacement`); no push.
