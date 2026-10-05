# H2-014: Define the Connection Disposition for Each Upstream Failure Case

## Severity

P2 policy gap. Split out of H2-008 on 2026-08-27; H2-008 closed with the
allocation and pool-selection guards only.

## Evidence

`ConnectionRef::mark_shutdown()` is now honored at stream allocation and at pool
selection (see
`skills/04-review/h2-grpc/regression-guards/h2-shutdown-connection-not-allocatable.md`). What is
still undefined is WHICH failures should raise it. Today only two do: an
upstream read timeout (`pingora-proxy/src/proxy_h2.rs`, the `ReadTimedout` arm)
and a `GOAWAY(NO_ERROR)` / `BrokenPipe` seen by `spawn_stream`. Malformed
upstream responses, write timeouts, and the various stream-local error paths
leave the connection eligible for reuse without an explicit decision that this
is correct.

## Required change

- Enumerate the upstream failure cases (malformed response, read timeout, write
  timeout, stream-local RST_STREAM, downstream-initiated cancel) and state the
  intended connection disposition for each: keep and reuse, mark shutdown, or
  close.
- Justify each choice — a stream-local failure on a healthy connection should
  keep the connection reusable; a connection-level or "peer is not behaving"
  failure should not.
- Apply the resulting policy in `proxy_h2.rs` where it diverges from the table.

## Acceptance tests

- A stream-local valid failure case still proves same-connection reuse for the
  next request where the connection is healthy.
- Each case that is declared "mark shutdown" is pinned by a test that fails if
  the mark is removed.

## Related files

- `pingora-proxy/src/proxy_h2.rs`
- `pingora-core/src/connectors/http/v2.rs`
