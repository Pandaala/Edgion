# Isolate DSL Sandbox Unit Tests from Network

## Problem and evidence

Two library unit tests in `edgion-gateway/src/plugins/http/dsl/tests/sandbox.rs`
start a loopback HTTP server and execute real http.call/reqwest requests:

- `test_http_call_headers_truncated` near line 148, MockServer start near 157.
- `test_nested_map_boundary` near line 199, MockServer start near 208.

They are loaded through the DSL cfg(test) module. This violates the no-network/no-real-server
unit-test rule in `skills/00-design-rules.md` section X and
`skills/05-testing/00-unit-testing.md` section 5. This is test isolation debt,
not evidence of a production defect.

## Required work and acceptance

- Preserve both distinct contracts: retained response-header prefix at max_map_size, and the fixed outer status/body/headers map retaining three fields even when max_map_size is two.
- Prefer an existing pure response-processing seam with non-network inputs. Do not add request-path copies or allocations solely for mocks.
- Neither unit scenario may listen or dial; retain explicit equivalent assertions.
- If real HTTP wiring requires new end-to-end coverage, record a separate deferred test task for later implementation; do not add or modify integration tests as part of this repair.
- Limit changes to these two scenarios and necessary minimal test support. Keep all existing integration bytes unchanged and run focused tests plus applicable project checks.

No matching DSL sandbox isolation task was found when this finding was recorded;
shared outbound-call and RequestMirror isolation tasks own different tests.
