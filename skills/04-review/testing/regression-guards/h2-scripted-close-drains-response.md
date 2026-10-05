---
name: h2-scripted-close-drains-response
description: Use when reviewing the transport test backend's response completion and GOAWAY scripts.
knowledge_type: regression-guards
---

# Scripted H2 normal closure drains the response

## Conclusion and scope

The test-only H2 origin must distinguish normal after-response closure from an
injected connection fault. `Goaway::DrainResponse` uses h2's graceful shutdown;
`Goaway::Abrupt(reason)` retains immediate fault shutdown, including the
mid-request `Behavior::Close` script whose code is also `NO_ERROR`.
An error code alone cannot identify the script's intended lifecycle.

## Rationale and evidence

P090-002 reproduced a 502 in the real consumer's
`retry_policy__goaway_bodyless_post__h1_to_h2`. Recorder evidence showed two
origin attempts: the first rejected the unprocessed stream; the second applied
one operation and queued a 200 response. The access log reported a read failure
on that second attempt. This did not demonstrate a missing retry.

In `edgion-tests/src/server/transport/h2.rs`, `send_response` and `send_data`
queue frames; recording `RespDone` there does not prove wire delivery.
`h2::server::Connection::abrupt_shutdown` (0.4.19) explicitly closes after
GOAWAY without waiting for outstanding streams. It could discard the successful
response while bytes were pending. `poll_accept` returning `Pending` is not
proof that flow-controlled DATA drained.

`AfterResponse::Close` and `AfterResponse::Goaway` with `NO_ERROR` now request
graceful draining. Error-code GOAWAY remains abrupt. The separate raw
`retry_wire::Fault::Goaway` still excludes the first accepted stream before
processing; changing that fault would invalidate the retry test.

## Verification and re-evaluation

The wire regression sets `SETTINGS_INITIAL_WINDOW_SIZE` to one byte, grants
the remaining credit after observing DATA, and requires the exact 16-byte
body, END_STREAM, NO_ERROR GOAWAY, and connection closure within five seconds.
It fails against the old handler with early EOF. A separate open-upload case
does not acknowledge PING and requires abrupt GOAWAY naming stream 1 and
connection closure, protecting the mid-request fault from accidental draining.

On 2026-10-05, `cargo test --locked -p edgion-tests --bin test_server` passed
all 58 tests after the fix. The real consumer's `HTTPRoute/Transport` then
passed all 133 executed cases, with 12 explicit unsupported-combination skips
and no failures, including all 65 retry-policy cases. Its shared run was
`tmp/integration_testing/testing_20261005_205059`; the residue check ran after
the supplementary suites. Production Gateway code, retry eligibility, and
Cargo.lock were unchanged.

Re-evaluate when changing h2 versions, response queueing, bridge flushing,
GOAWAY scripts, or the recorder's distinction between queued and delivered data.

## Resolution references

- P090-002 — test-backend closure defect corrected on 2026-10-05; synchronization
  publication remains a separate task gate.
