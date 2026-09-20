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
  `"idle timeout"` err_log. The EGC publication path resolves the value with
  `resolve_tcp_idle_timeout(spec.tcpTimeout)`; each connection pins it before
  route processing and passes it to the shared relay.

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

The original B0196 verification above used in-memory streams. The
`add-l4-duplex-backpressure-integration` follow-up adds the shared-topology
`TCPRoute/DuplexBackpressure` suite, covering TCP, TLS termination, and TLS
passthrough in both directions. Current execution results are recorded below;
the historical unit-test results are not fresh integration evidence.

### Transport-level regression boundary

The fixture lives in `edgion-tests/src/server/l4_duplex.rs`; the suite is
`edgion-tests/src/client/suites/l4_duplex.rs`. Both use the bounded wire helpers
in `edgion-tests/src/server/l4_duplex_protocol.rs`. The fixture owns its listeners
inside the ordinary `test_server`, and the scenarios use the canonical shared
Controller/Gateway topology and standard route resources.

Each stalled sender retains a 16 KiB working buffer while attempting a bounded
32 MiB position-dependent payload. The receiver explicitly withholds reads.
The case requires a nonzero, incomplete send count to remain unchanged before
proceeding; fitting the entire payload in transport buffers fails the test.
Reverse pulses must cross the gateway throughout a three-second unchanged-send
window, longer than the configured two-second idle window. Any late TCP window
update resets that observation window; a ten-second deadline bounds the attempt.
Explicit drain then verifies
every byte and the terminal marker. No half-close is used to delimit data.

Separate fully stalled cases require exactly one connection-close record,
correlated by listener and direct peer port, with TCP `status=IdleTimeout` or
TLS `err_log="idle timeout"`, followed by byte-stream closure. Only the copied
integration gateway config enables the existing TCP log sink. The source
configuration and production relay behavior are unchanged.

The fixture limits connections, command queues, payload size, and exchange
lifetime; the client requests cleanup even after an exchange failure and checks
that the fixture registration disappears. Network tests establish end-to-end
backpressure, not a particular `poll_flush(Pending)` event. Exact flush-state
and scheduling boundaries remain covered by the in-memory relay tests.

Verification on 2026-09-09: workspace fmt/check/Clippy, agent-doc, SSA-force,
metrics-inventory, and Pingora-source guards passed; Clippy retains pre-existing
warnings outside the new files. All 21 test-client capability/registration tests
passed, and independent code review returned LGTM. The complete canonical command
`./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r TCPRoute -i DuplexBackpressure`
passed all 12 cases in 40.42 seconds with zero infrastructure errors and exit 0
(run `testing_20260909_155048`). This closes the test gap tracked by
`add-l4-duplex-backpressure-integration` / D0110 without changing the production
relay or immediate-EOF policy.

The initial network run exposed the short-plateau assumption corrected above.
Two subsequent attempts were interrupted by competing worktree lifecycle
operations; those partial runs are not the passing evidence. The canonical local
shared topology intentionally permits only one owner per host environment, and a
competing worktree run may clear the fixed-port test processes; see
the [single-owner takeover decision](../../testing/intentional-design/local-shared-topology-single-owner-takeover.md).
The successful exclusive run does not establish isolation between concurrent
workers because that concurrency mode is unsupported.
Re-evaluate when stream wrappers, readiness/waker behavior, buffer state,
transport buffer sizes, idle configuration, or shutdown policy changes.

## Behavior contract preserved by the helper

- Two independently-owned 8 KiB heap buffers.
- `Ok(0)` (clean EOF) on either side breaks without invoking `on_error`.
- Byte counters are recorded *before* the matching write, so they reflect bytes
  read from the source even if the subsequent write fails.
- Client-facing direction is `flush`-ed after every upstream → client write;
  upstream direction is not flushed (matches prior behavior across all three
  call sites).
