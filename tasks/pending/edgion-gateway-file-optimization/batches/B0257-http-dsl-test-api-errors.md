# B0257 — HTTP DSL API-error Tests

## Scope

- Pipeline state: in-progress; refreshed analysis passed the value gate after B0256.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `f2d39a75`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/api_errors.rs`.
- Target blob: `61b00969533f35680ec9fb411e0d5945d4d4aec9`.
- Read scope includes the full test file, every exercised builtin/runtime error path, owning implementations, fixtures, adjacent tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only test coverage.
- Twelve production session-mutation wrappers map failures to builtin-specific `RuntimeError::ApiError.function`, but only seven have explicit failure-path coverage across the DSL suite. The target's exhaustive claim omits `req.append_header`, `req.remove_header`, `resp.set_header`, `resp.append_header`, and `resp.remove_header`.
- Add one target-local function-name assertion helper, route the four existing target assertions through it, and add one independent exact mock expectation/test per missing builtin. Do not assert dynamic session error text.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/api_errors.rs` only. Preserve all production behavior, failure policy, diagnostics, protocol/session semantics, and integrations.
- Target and full DSL test filters plus campaign gates are required. D0130 can absorb the stale panic-knowledge statement later without a new issue.

## Implementation and Review

- Added a target-local assertion helper that checks only the static `ApiError.function` label and routed the four existing assertions through it.
- Added five independent exact-argument, exactly-once mock failure tests for request append/remove header and response set/append/remove header operations.
- Independent final review: LGTM, no Must Fix or Should Fix. All 12 production session/context mutation wrappers now have explicit error-label coverage across the DSL suite; dynamic session detail remains unasserted and undisclosed.
- D0130 was refined to name the stale chrono-display panic guard explicitly; no duplicate task was created.

## Verification

- Target tests: 9/9; full DSL tests: 328/328; Gateway locked all-target compile; format and diff checks passed.
- Coordinator: `cargo fmt --all`; locked workspace check; locked workspace Clippy (exit 0 with pre-existing warnings); agent docs; SSA force; metrics inventory; Pingora source policy; Gateway no-tracing; English-only; diff check; and frozen paths all passed.
- Frozen integration manifest: 3,231 paths unchanged.
