# H2-010: Make H2 Regression Evidence Deterministic and Branch-Specific

## Severity

P2 test-evidence gap.

## Evidence

- The staged `poll_trailers()` error latch is tested only in a fixture where
  the pre-existing terminal-HEADERS synthetic error can satisfy the assertion.
- The raw response helper waits for a HEADERS frame on stream 1 but does not
  verify `END_HEADERS` or the full header block; its flush notification proves
  bytes were written, not that the client processed the reset.
- The registration-race test depends on timing rather than a deterministic
  scanner barrier.

## Required change

- Add a no-reset invalid-trailer fixture that makes `poll_trailers()` itself
  return `Err`, then prove later `read_trailers()` remains failed.
- Parse HEADERS flags and CONTINUATION frames when a helper claims to wait for
  a complete request header block.
- State the exact guarantee of the flush channel, and add a client-observable
  barrier when the test requires processing rather than delivery.
- Replace timing-only registration assertions with an explicit test hook or
  barrier.

## Acceptance tests

- Removing the staged poll-error latch makes its dedicated regression fail.
- Raw-wire pre-poll tests fail if the response/reset is delayed until after the
  first body poll.
- Registration race test fails deterministically when lock serialization is
  removed.

## Related files

- `pingora-core/src/protocols/http/v2/client.rs`
- `pingora-core/src/protocols/http/v2/end_stream_watch.rs`
