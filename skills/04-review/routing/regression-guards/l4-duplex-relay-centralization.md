---
name: l4-duplex-relay-centralization
description: Shared bounded, independently polled TCP and TLS relay; hook ownership, backpressure progress, accounting, flush, and EOF regression boundaries.
knowledge_type: regression-guards
---

# L4 Duplex Relay — Centralized in `l4_duplex.rs`

**Audit date**: 2026-05-26
**Linked issue**: `path-norm-nonhttp-routes` F-010 (closed by extracting the shared helper).

## Conclusion

All L4 (TCP / TLS-terminated / TLS-passthrough) bidirectional relay traffic now
flows through a single helper:

- `pub(crate) async fn bidirectional_relay(downstream, upstream, hooks, idle_timeout)` in
  `edgion-gateway/src/routes/l4_duplex.rs`
- Trait `DuplexHooks { record_sent, record_received, on_error }` plugs in the
  per-context bits.
- Error variants: `DuplexError::{UpstreamWrite, DownstreamRead, DownstreamWrite,
  DownstreamFlush, UpstreamRead}`.

Concrete hook impls:

- `TcpDuplexHooks` — in `edgion-gateway/src/routes/tcp/edgion_tcp.rs`. Maps errors
  to `TcpStatus::*`. Note: `DownstreamFlush` collapses to
  `TcpStatus::DownstreamWriteError` — this matches pre-refactor TCP behavior
  (which never distinguished flush from write failure).
- `TlsDuplexHooks` — in `edgion-gateway/src/routes/tls/mod.rs`, shared by both
  `proxy.rs` (terminate) and `passthrough_proxy.rs`. Writes free-form strings
  into `ctx.err_log`. `DownstreamFlush` produces `"downstream flush err"`.

## Centralization scope

The historical duplicated relay loops were removed. Each `duplex(...)` method on the
three proxies is now a one-line dispatch to `bidirectional_relay`. If a fourth
L4 proxy variant appears, route it through the same helper instead of cloning
the loop body.

## When changing the relay

Changes that need to land in **one** place now (instead of three): buffer size,
read/write ordering, flush policy on the client-facing side, EOF handling, new
error categories, timeouts. Cross-check:

- `l4_duplex.rs` — the skeleton.
- `TcpDuplexHooks::on_error` — mapping to `TcpStatus`.
- `TlsDuplexHooks::on_error` — mapping to `err_log` strings.
- If you add a new `DuplexError` variant, every hook impl must cover it
  (compiler-enforced via the exhaustive `match`).
- Idle timeout (landed 2026-06-02, finding fr-l4-02): `bidirectional_relay` takes
  `idle_timeout: Option<Duration>` and is bounded by a single idle clock reset on
  any byte progress (read or partial write) in either direction.
  `DuplexError::IdleTimeout` maps to `TcpStatus::IdleTimeout` and the TLS
  `"idle timeout"` err_log. Value comes from `resolve_tcp_idle_timeout(spec.tcpTimeout)`.

## Independent progress under backpressure

B0196 found that centralization alone did not provide full-duplex progress:
the read-select arm awaited its whole write and downstream flush, preventing
reverse reads while either operation was pending. A peer waiting for an ACK
before draining its receive window could therefore deadlock the relay until
idle timeout; disabling idle left the deadlock unbounded.

`RelayDirection::poll_step` retains one bounded buffer and write cursor per
direction. The single relay future polls both directions on each sweep,
alternates their ordering, and self-wakes after at most 32 progressing sweeps.
When both directions are pending, actual I/O or the shared timer supplies the
wakeup. No spawned pumps, stream-split locks, extra payload queues, or partial
write cancellation/replay are involved. A pending downstream flush blocks only
that direction's next read, not client-to-upstream traffic.

Regression tests `blocked_upload_does_not_block_download` and
`blocked_download_does_not_block_upload` both failed against the prior relay:
after one write filled its output window, a reverse ACK remained unread.
The tests use in-memory duplex streams and explicit polling, not network timing.
Additional relay tests cover executor-driven wakeups, retained bytes across
partial writes, idle boundaries, all error classes, write-zero, and EOF.
The three earlier `write_all_idle` unit tests are replaced at the relay boundary
by `relay_completes_when_reader_drains`, the two `blocked_*_expires_without_early_timeout`
tests, and `partial_writes_reset_idle_and_preserve_payload`.

This is an in-memory relay verification boundary, not a fresh TCP/TLS integration
run. Transport-level coverage remains deferred in
`tasks/todo/add-l4-duplex-backpressure-integration.md`. Re-evaluate when stream
wrappers, readiness/waker behavior, buffer state, or shutdown policy changes.

## Behavior contract preserved by the helper

- Two independently-owned 8 KiB heap buffers.
- `Ok(0)` (clean EOF) on either side breaks without invoking `on_error`.
- Byte counters are recorded *before* the matching write, so they reflect bytes
  read from the source even if the subsequent write fails.
- Client-facing direction is `flush`-ed after every upstream → client write;
  upstream direction is not flushed (matches prior behavior across all three
  call sites).
