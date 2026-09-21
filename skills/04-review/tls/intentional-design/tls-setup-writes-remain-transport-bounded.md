---
name: tls-setup-writes-remain-transport-bounded
description: Use when reviewing deadlines around TLSRoute PROXY protocol and passthrough ClientHello setup writes before the shared L4 relay starts.
knowledge_type: intentional-design
---

# TLS setup writes remain transport-bounded

## Conclusion and scope

TLSRoute does not add an application-level deadline around the upstream setup
writes performed after a successful TCP connect and before
`bidirectional_relay` starts. This applies to the PROXY protocol v2 header in
terminate and passthrough mode and to replaying the bounded inspection buffer in
passthrough mode.

This is an accepted low-severity tradeoff, not a guarantee that an asynchronous
socket write can never remain pending. The setup payload is bounded and is sent
on a newly connected upstream socket, so no concrete production hang has been
established. The configured `connectTimeout` owns the TCP handshake and
`idleTimeout` owns relay inactivity; neither field implicitly owns this setup
phase. An explicit zero value retains only the documented opt-out semantics of
its existing phase and does not create a separate setup policy.

## Rationale and evidence

The current call chains are:

- Terminate and passthrough both call the canonical
  `routes/tls/proxy.rs::send_proxy_protocol_v2` after a successful connect.
  The helper writes and flushes PP2 without adding a separate deadline.
- Terminate enters `bidirectional_relay` after the helper succeeds.
- Passthrough calls `replay_initial_bytes` after the helper succeeds, then
  enters `bidirectional_relay`.
- `replay_initial_bytes` owns at most the bytes drained into
  `sni_parser::MAX_CLIENT_HELLO_LEN` (16 KiB), forwards them exactly once, and
  updates `TlsUpstreamInfo.bytes_sent` only after the complete write and flush.

The pinned Pingora revision `518f727` gives a newly connected raw L4 `Stream` a
zero-capacity `BufStream` write buffer. Its `AsyncWrite` implementation delegates
to Tokio's TCP stream and does not apply `PeerOptions.write_timeout`; Tokio TCP
`poll_flush` is an immediate no-op. A socket write can still return `Pending`
when the kernel cannot accept bytes, but an always-pending test writer proves
only that abstract condition. It does not establish that the bounded setup data
on a fresh production TCP connection can exhaust the actual transport buffers
and remain stuck.

The canonical timeout contracts deliberately keep these phases separate:

- `EdgionGatewayConfig.spec.tcpTimeout.connectTimeout` bounds upstream TCP
  establishment.
- `idleTimeout` starts in the shared relay and resets on byte progress in either
  direction.
- The passthrough ClientHello read has its own absolute downstream deadline.

No project or protocol rule currently requires a fourth setup-write deadline.
The residual risk was explicitly accepted when closing
`investigate-tls-setup-write-deadlines` on 2026-09-19.

## Alternatives and limits

Reusing `idleTimeout` was rejected for this narrow issue because it would expand
the documented relay-only field into a different phase. Wrapping `write_all` in
one absolute timeout would also differ from relay idle semantics: partial write
progress would not reset the clock. Reusing `connectTimeout` would incorrectly
extend a field documented as bounding the TCP handshake.

A dedicated setup timeout would require an explicit owner, default, zero-value
behavior, error vocabulary, configuration publication, and progress semantics.
That design cost is disproportionate without a real transport reproduction.
Adding a timeout solely because a synthetic writer can remain pending would
test the wrapper's premise rather than the production failure mode.

This decision does not claim that operating-system TCP behavior supplies a
portable hard deadline. It accepts the absence of such an application-level
deadline only for the current bounded setup payload and raw L4 transport.

## Verification and re-evaluation

The closure investigation inspected the current TLS call chains, the resolved
L4 timeout publication, Cargo's pinned Pingora source, and Tokio's TCP
`AsyncWrite` implementation. A local macOS socket probe accepted 64 KiB on a
fresh connection even when a smaller socket buffer was requested; that probe is
supporting evidence only and is not a cross-platform guarantee. The later B0068
unification added generic writer tests for exact PP2 bytes and write/flush
failure accounting without changing this timeout decision.

Re-evaluate when any of the following occurs:

- a production or transport-level test reproduces a sustained pending setup
  write with the current bounded payload;
- the setup payload bound grows or additional pre-relay writes are introduced;
- Pingora adds or inherits a raw L4 write deadline;
- the product defines a deadline covering all post-connect, pre-relay work; or
- a proposed implementation can preserve progress-based idle semantics without
  silently changing `connectTimeout` or `idleTimeout`.

## Resolution references

- `investigate-tls-setup-write-deadlines` — accepted tradeoff, 2026-09-19;
  investigation task formerly at
  `tasks/todo/investigate-tls-setup-write-deadlines.md`.
