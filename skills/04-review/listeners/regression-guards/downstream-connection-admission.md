---
name: downstream-connection-admission
description: Use when reviewing downstream connection counting, the process-wide max_downstream_connections cap, the ConnectionLimit stream plugin, or the Pingora admission-permit seam.
knowledge_type: regression-guards
---

# Downstream connection admission and exact live counts

## Conclusion and scope

The Gateway counts live downstream connections exactly and can cap them per
process, per physical listener, per TCP route stage, and per UDP listener's
sessions. Both caps are off by default.

- **Process cap**: `server.max_downstream_connections` (gateway-local YAML / CLI,
  `NonZeroU64`, startup-only). One `ConnectionCounter` shared by every Pingora
  listener; admission uses a CAS loop, so a rejected attempt never changes the
  count and the cap is never exceeded.
- **Listener cap**: a Gateway-attached `ConnectionLimit` stream plugin. Each
  physical listener owns an unlimited counter; the socket's `fetch_add` ordinal is
  passed to the Stage-1 chain in `StreamContext::active_connections`, and the
  plugin denies `ordinal > maxConnections`.
- **Route caps**: TCPRoute/UDPRoute-attached `ConnectionLimit`. `EdgionTcpProxy`
  owns a route-stage counter whose slot lives in `handle_connection` for the whole
  relay; `EdgionUdpProxy` owns a session counter whose slot lives in
  `ClientSession`.

Admission order on a listener (`runtime/server/downstream_connections.rs`,
`ListenerAdmissionFilter::admit`): fatal PROXY-policy guard, process cap,
listener slot, Stage-1 chain. All counters are owned by objects created once per
listener at startup, so config reloads never reset or double-count them.

## Rationale and evidence

Pingora's `ConnectionFilter::should_accept` returns a `bool` and nothing observes
the end of the connection (also cloudflare/pingora#994). The Edgion Pingora fork
adds `ConnectionFilter::admit`, which may return an opaque `ConnectionPermit`;
`ListenerEndpoint::accept` attaches it to the accepted l4 `Stream`
(`pingora-core/src/listeners/l4.rs`, `protocols/l4/stream.rs`). The l4 stream is
dropped on every exit path (handshake failure or timeout, PROXY failure,
application end, keep-alive end, H2 close, shutdown), so the permit's slots are
released exactly once. Rejection happens before PROXY parsing and the TLS
handshake. A cancelled admission future (shutdown `select!`) drops its local
slots together with the accepted socket.

## Alternatives and limits

Rejected alternatives: reusing `Stream.tracer` (its `boxed_clone` would release
twice), a separate listener tracer (check and increment in two places), and an
Edgion `ServerApp` wrapper (counts only after the TLS handshake).

Review invariants and accepted limits:

- `ConnectionCounter::acquire` ignores the limit (debug-asserted); only the
  process counter is limited and uses `try_acquire`. The global slot is taken
  before the listener slot so a process-cap rejection never inflates a listener
  ordinal; no test can observe the transient listener slot, so review this order.
- The route-stage and UDP session counters belong to the proxy of one physical
  listener, not to a route, and are never shared between attachments of one
  `EdgionStreamPlugins` resource.
- The bridge accepts a non-inet peer without running the chain; Edgion binds no
  UDS data-plane listener.
- Retirement wakes the UDP session's receiver. Its slot is released when the
  last `Arc<ClientSession>` drops.
- `edgion_downstream_connections_active` counts admitted listener sockets (after
  a successful chain); internal counters also include sockets still in admission.
- Memory-pressure shedding is tracked in `tasks/todo/gateway-overload-manager.md`;
  per-client concurrency limits in gw-plugrt-05. Idle HTTP/2, slow HTTP/1 headers
  and the h2c preface peek can hold a slot without a deadline; tracked in
  `tasks/todo/downstream-slot-hold-deadlines.md`.

Operator-facing caveats (slot holding while idle or in the handshake, the
shared process cap, UDP spoofing, per-pod scope, plugin ordering, uncounted
admin ports) are canonical in
[gateway-config](../../../../docs/en/ops-guide/configuration/gateway-config.md)
and the
[ConnectionLimit user guide](../../../../docs/en/user-guide/tcp-route/stream-plugins.md#connectionlimit-concurrent-connection-cap).

## Verification and re-evaluation

Unit tests: `edgion-gateway/tests/unit/runtime/server/downstream_connections_test.rs`
(CAS cap under threads, ordinals, fatal-first ordering, release on deny and
cancellation, gauge balance and the `global` reason under a local recorder,
per-listener counters sharing the process counter, listener filter wiring via
`admit_with` with an accepting chain),
`proxy_protocol_policy_test.rs` (a tripped policy rejects before counting or
running the chain), `plugins/stream/connection_limit/plugin_test.rs` (including the
listener/route reason mapping),
`connection_filter_bridge_test.rs` and `routes/tcp/edgion_tcp_test.rs` (ordinal
reaches the plugin), UDP session slot tests in `routes/udp/edgion_udp_test.rs`,
config parse tests, resource and Controller validation tests, and the fork's
`listeners::` tests (permit lifetime, non-inet peer admitted with `None`, permit
kept through a PROXY handshake and released when it fails).
Integration: `Gateway/StreamPlugins` (`tcp_connection_limit_listener`) and
`TCPRoute/StreamPlugins` (`tcp_stream_plugin_conn_limit_route`).

Recorded 2026-11-07 on `feature-10-28` with the fork as a local path patch:
`cargo test -p pingora-core --features connection_filter --lib listeners::`
(29 passed); targeted `cargo test -p edgion-gateway --lib` filters for the
admission, stream-plugin, TCP/UDP route and config modules (all passed);
`cargo clippy --workspace --all-targets` and `cargo check --workspace
--all-targets` clean for the changed code. The global cap has no integration
scenario because the shared topology runs one process configuration.
Re-evaluate when Pingora changes accept ownership, stream wrapping, or the
`ConnectionFilter` contract, or when a listener is created after startup.

## Resolution references

- arch-02 (no process-level overload protection): connection admission
  implemented 2026-11-06; the finding closes once the fork commit is published
  and Edgion's lockfile points at it. Memory-pressure shedding is deferred to
  `tasks/todo/gateway-overload-manager.md`.
- gw-plugrt-05: per-listener concurrency cap resolved here; per-IP limiting remains open.
