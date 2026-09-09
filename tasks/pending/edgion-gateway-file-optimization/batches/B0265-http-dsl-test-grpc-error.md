# B0265 — HTTP DSL gRPC-error Tests

## Scope

- Pipeline state: complete; target-only implementation, independent final review, and coordinator gates passed.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `420b0905`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/grpc_error_tests.rs`.
- Target blob: `8913f5000ecbb29a1e64fdc7229f1429b31f568d`.
- Read scope includes the complete test file, gRPC error builtin/compiler/VM/result mapping, downstream terminal response semantics, adjacent DSL tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; simple, target-only unit coverage.
- Add exact `grpc_error(7, nil)` coverage proving the public DSL-to-result translation uses `message: None`, never a literal `nil` string.
- Replace the far-out `99` invalid-code case with the two exact integer boundaries `-1` and `17`, both normalizing to gRPC INTERNAL code 13 while preserving the message.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/tests/grpc_error_tests.rs` only. Preserve stack order, valid codes, terminal semantics, independent HTTP/gRPC status, canonical message encoding, logging, and frozen integration tests.
- No new integration test is needed. D0138 separately corrects stale bilingual response-stage documentation.

## Implementation and Review

- Added exact `nil`-message translation coverage and replaced the far-out invalid code with lower/upper boundary cases `-1` and `17`.
- Independent final review: LGTM with no findings. It independently traced compiler/VM stack order, nil translation, both integer boundaries, message preservation, and exact target-only scope.

## Verification

- Implementer: target 4/4 and full Gateway DSL 334/334 passed; Gateway all-target check, format, diff, English-only, and Gateway no-tracing checks passed.
- Reviewer: target 4/4 and full Gateway DSL 334/334 passed; format, diff, and frozen-path checks passed.
- Coordinator full workspace checks passed: format, locked workspace all-target check and Clippy, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen integration guards.
- Frozen integration manifest: 3,231 paths unchanged.
