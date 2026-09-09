# B0266 — HTTP DSL Outbound-call Tests

## Scope

- Pipeline state: analysis-in-progress; queued read-only behind active B0265.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `420b0905`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs`.
- Target blob: `83f3791ef2d06f256a3994b22c337076c3d6c730`.
- Read scope includes the complete test file, DSL `http.call` validation/runtime/client/egress/TLS/body-cap paths, response-stage bridging, adjacent tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Pending analyst report. This candidate cannot enter implementation/review before B0265 commits and must refresh against that HEAD.
