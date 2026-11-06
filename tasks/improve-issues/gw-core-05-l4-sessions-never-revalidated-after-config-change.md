# gw-core-05: Established L4 connections and UDP sessions are never re-validated after route, backend or policy changes

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/src/routes/udp/edgion_udp.rs:37` (`SESSION_TIMEOUT`), `:265-276` (established-session fast path), `:198-232` (single serve task), `:625-660` (idle-only expiry)
  - `edgion-gateway/src/routes/tcp/edgion_tcp.rs:255-290`, `edgion-gateway/src/routes/tls/proxy.rs:429-451`, `edgion-gateway/src/routes/tls/passthrough_proxy.rs:347-374` (relay after the route generation is dropped)
  - `edgion-gateway/src/routes/l4_duplex.rs:185-240` (relay bounded only by idle timeout; default 1h via `DEFAULT_TCP_IDLE`)

## Summary
TCPRoute, TLSRoute (terminate and passthrough) and UDPRoute evaluate route selection, ReferenceGrant
admission, stream-plugin policy (for example IP restriction) and backend endpoint selection only when a
connection or UDP session is created. After that, the relay runs with no link to the configuration
generation. Deleting the route, revoking the ReferenceGrant, tightening the IP allow-list or removing the
backend endpoint has no effect on existing flows. TCP and TLS flows live until either side closes or the
idle timeout fires (1h by default). UDP is worse: a session is keyed by client address and refreshed by
every datagram, and it keeps sending to its pinned upstream address forever as long as the client sends at
least once per 60s. After a backend pod is deleted, an active UDP client is blackholed indefinitely.

## Current behavior (evidence)
- UDP fast path (`edgion_udp.rs:265-276`): "Route/backend changes take effect when the session expires
  (SESSION_TIMEOUT) and a new one is created". Expiry (`cleanup_expired_sessions`, `:637-660`) is purely
  `now - last_activity > 60s`, and every successful send refreshes `last_activity`.
- Nothing outside `edgion_udp.rs` references `client_sessions`. The UDP route manager, endpoint discovery and
  plugin publication have no hook to retire sessions.
- TCP/TLS: `tcp-connection-snapshot-ownership.md` and the TLS ownership guards confirm that the selected unit
  is dropped before connect and relay. `bidirectional_relay` is then bounded only by `idle_timeout`, which
  resolves to `DEFAULT_TCP_IDLE = 3600s` (`edgion-resources/.../edgion_gateway_config.rs:480-500`). There is
  no generation token or cancellation path from config handlers to live relays.
- UDP data path is a single task per Listener: `serve` (`:198-232`) awaits `handle_client_packet` inline for
  every datagram. That includes new-session work (publication snapshot, async stream-plugin evaluation,
  `UdpSocket::bind`, `log_udp(..).await` on every rejection). All clients of a Listener are serialized
  behind one task, and a flood of denied or new-source datagrams stalls established sessions' forwarding.

## Industry comparison
- **Envoy UDP proxy**: sessions are per 5-tuple. The filter registers a cluster *member update callback*
  and tears down every session whose host was removed (`host_to_sessions_`). UDP listeners use
  `SO_REUSEPORT` with one socket per worker, so datagram processing scales across cores.
- **Envoy TCP proxy / Istio**: a route or filter-chain change through LDS drains connections on the old
  filter chain (`drain_timeout`, then close). An RBAC or authorization update can close existing
  connections. Endpoint removal plus health-check failure closes upstream connections
  (`close_connections_on_host_health_failure`, `close_connections_on_host_set_change`).
- **NGINX stream**: a reload drains old workers, so established connections end within
  `worker_shutdown_timeout`.

## Impact
- Security: revoking access at L4 by deleting a TCPRoute or TLSRoute, revoking a ReferenceGrant or adding a
  stream IP deny does not take effect for established flows for up to 1h (TCP/TLS), or indefinitely for
  active UDP clients.
- Availability: UDP clients pinned to a deleted pod are blackholed until they go silent for 60s. For
  chatty protocols such as DNS forwarders, game servers and telemetry, that means never. TCP and TLS
  passthrough hold connections to retired backends until those backends close them.
- Throughput: UDPRoute forwarding is capped at one task per Listener regardless of core count. Admission
  bursts, including spoofed-source floods within `MAX_UDP_SESSIONS`, add head-of-line latency to all
  established sessions.

## Suggested direction
1. **Generation-aware revocation (TCP/TLS/UDP)**: give each published L4 route unit a
   `CancellationToken` (or a `watch` generation) owned by the route table entry. Fire it when the unit is
   removed, its parent attachment or ReferenceGrant admission is withdrawn, or its stream-plugin scope
   changes. `bidirectional_relay` and the UDP session loops already use `select!` and can add one more
   branch. Make the behavior a drain, closing after an optional `drainTimeout`, rather than an instant
   reset. This mirrors Envoy's filter-chain drain.
2. **Endpoint-removal teardown**: subscribe L4 sessions to endpoint-set changes for their service. For UDP,
   keep a `upstream_addr → sessions` index and retire matching sessions on removal, as Envoy does. For TCP,
   optionally close on host removal behind a policy flag (Envoy's
   `close_connections_on_host_set_change`).
3. **UDP data path**: bind `N` `SO_REUSEPORT` sockets per UDP Listener, one serve task each. Move
   new-session admission off the receive loop (spawn per new peer with a bounded admission semaphore). Make
   rejection logging non-blocking (`try_send`).

Option 1 alone closes the security gap. Option 2 fixes UDP blackholing. Option 3 is independent and lower
priority, since UDPRoute is not yet an advertised conformance feature.

## Notes
- Checked `skills/04-review/routing/regression-guards/{tcp,tls-passthrough,tls-terminated}-connection-ownership.md`,
  `listeners/regression-guards/udp-session-generation-retirement.md`, `udp-process-shutdown.md`,
  `routing/regression-guards/l4-backend-ref-denied-fail-closed.md` and
  `cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md`. They cover memory
  ownership, retirement races and *new*-connection denial, and the last explicitly accepts "route/backend
  changes take effect when the session expires" as a CPU optimisation. None evaluates revocation or
  endpoint-removal semantics against Envoy's behavior, and no `tasks/` item tracks this.
- The listener-port freeze (`listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md`)
  is a separate, accepted boundary and is not challenged here.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P2 unchanged)
- `edgion-gateway/src/routes/udp/edgion_udp.rs:265-276`: established sessions bypass route/backend/policy and send to the pinned `upstream_addr`; the comment states changes apply only on session expiry.
- `edgion_udp.rs:637-660`: expiry is purely idle-based (`SESSION_TIMEOUT` = 60s at `:37`), refreshed by each send, so an active client never re-validates.
- `edgion_udp.rs:213-221`: `handle_client_packet` (including new-session admission) is awaited inline in the single per-Listener `serve` loop.
- `skills/04-review/cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md` justifies the fast path as a CPU fix (the old result was discarded anyway) but does not evaluate revocation semantics; no page accepts L4 non-revocation.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `edgion-gateway/src/routes/udp/edgion_udp.rs` has no diff: `SESSION_TIMEOUT` at `:37`, fast-path comment at `:271`, idle-only expiry at `:643`.
- `routes/tcp`, `routes/tls` and `routes/l4_duplex.rs` have no diff; no cancellation/generation token reaches live relays.
