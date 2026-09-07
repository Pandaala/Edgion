---
name: request-mirror-unit-tests-without-network
description: Use when changing RequestMirror arming tests or the detached writer transport boundary.
knowledge_type: regression-guards
---

# RequestMirror unit tests isolate the writer transport

## Conclusion and scope

RequestMirror unit tests exercise the real sampling, endpoint selection, semaphore,
header preparation, committed snapshot, streaming channel, cancellation, and outcome
logic. Only the writer's HTTP exchange is replaced by an in-memory consumer under
`cfg(test)`. The production writer retains the dedicated canonical HTTP client,
combined deadline, and immediate response drop after headers.

The test-only event channel observes prepared request fields, streamed chunks, and
the final status and counters. Tests wait for the final event and verify that the
concurrency permit was released; the streaming-completion case also joins the writer.
These fields and the transport substitution do not exist in production builds.

## Rationale and evidence

The canonical testing boundary is [design rules, section X](../../../00-design-rules.md#x-testing-discipline):
unit tests do not hit a network, real database, or real server.

`edgion-gateway/src/plugins/http/request_mirror/plugin.rs` previously spawned the
real reqwest writer from four arming tests, using `192.0.2.10:8080` as an endpoint.
A TEST-NET address does not prevent socket attempts. The original D0067 report
overstated actual network activity: spawning a future in a current-thread Tokio
test does not prove it was polled before runtime teardown. The confirmed defect
was the absence of a no-dial boundary and deterministic writer-lifecycle checks.

The corrected tests retain all four arming cases: header END_STREAM, nonterminal
headers, unknown header termination, and rejected installation after body-stage
freeze. Additional assertions cover the prepared endpoint/method/headers,
streamed bytes, `ok` versus `body_abandoned`, permit ownership/release, and
request PluginLog decisions. Separate cases cover disabled sampling, saturated
concurrency, and an immutable committed snapshot with independently owned framing.

The fixed EndpointSlice fixture is published once with `std::sync::Once`. During
verification, concurrent replacement of the same fixture made the frozen-stage
test exit with `mirror resolve error`; its strengthened PluginLog assertion caught
the wrong path. One-time publication removes this fixture-induced race without
mocking endpoint selection.

## Alternatives and limits

Do not substitute a local HTTP server or a different unreachable address. Neither
provides unit-test isolation. Do not skip `run_request` or disable sampling in the
positive cases, which would bypass the arming behavior under test.

The in-memory consumer is not evidence for reqwest wire framing, DNS, TLS, transport
error classification, or access-log sink delivery. Mirror logging remains disabled
in the test fixture; final outcome assertions cover the state used by logs/metrics,
not a running log pipeline. Existing integration tests remain unchanged, including
`edgion-tests/src/client/suites/edgion_plugins/request_mirror/` and body-plugin mirror
scenarios. See also [committed snapshot ownership](request-mirror-committed-body-snapshot.md).

## Verification and re-evaluation

Verified on 2026-09-07:

- `cargo test -p edgion-gateway --lib plugins::http::request_mirror::`: 11 passed.
- The resulting unit-test executable passed 20 additional runs of the same
  11 tests with `--test-threads=8` (220 test executions).
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy reported existing
  workspace warnings; the changed module's reported warnings were on unchanged code.
- Agent-doc, SSA-force, metrics-inventory, Pingora-source-policy, and gateway
  no-tracing guards passed. Independent read-only review returned LGTM.
- Existing integration tests were not changed or rerun; the full workspace
  test suite was not run.

Re-evaluate when the writer transport,
body ownership, cancellation ordering, or test runtime changes. New arming tests
must attach a writer observer and await its outcome rather than depend on runtime
teardown to cancel unobserved work.

## Resolution references

- D0067 / `isolate-request-mirror-unit-tests-from-network.md`: fixed test-isolation
  defect; originated in B0102.
