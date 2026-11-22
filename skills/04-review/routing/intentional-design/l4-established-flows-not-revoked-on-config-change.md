---
name: l4-established-flows-not-revoked-on-config-change
description: Use when reviewing whether TCPRoute, TLSRoute or UDPRoute changes (route deletion, ReferenceGrant revocation, stream-plugin policy, backend selection) should terminate already established L4 connections or UDP sessions.
knowledge_type: intentional-design
---

# L4 established flows are not revoked by configuration changes

## Conclusion and scope

Plain TCP, TLS-terminated and TLS-passthrough connections, and UDP client
sessions, evaluate route selection, parent attachment, ReferenceGrant admission,
stream-plugin policy and backend selection once, when the connection or session
is admitted. A later configuration change applies to new connections and new
sessions only. An established TCP/TLS relay ends when either side closes or the
shared idle timeout fires (`tcpTimeout.idleTimeout`, default one hour). An
established UDP session ends after 60 s of inactivity.

This is an accepted choice for configuration changes. It does not cover
endpoint removal for UDP: a session whose pinned endpoint is no longer published
by service discovery is retired, see
[UDP endpoint-removal retirement](../../listeners/regression-guards/udp-endpoint-removal-retirement.md).

## Rationale and evidence

gw-core-05 (2026-11) reported that deleting a route, revoking a ReferenceGrant or
tightening a stream IP policy does not affect established flows. The behavior
was confirmed in current source: `routes/tcp/edgion_tcp.rs`,
`routes/tls/proxy.rs` and `routes/tls/passthrough_proxy.rs` drop the route
generation before entering `bidirectional_relay`, and the UDP fast path in
`routes/udp/edgion_udp.rs::handle_client_packet` forwards to the pinned upstream
without route or policy evaluation.

The default proxy model in comparable products is the same:

- NGINX stream reload leaves established connections on old workers until they
  finish; `worker_shutdown_timeout` is unset by default.
- HAProxy reload leaves established connections on the old process; `hard-stop-after`
  and `on-marked-down shutdown-sessions` are opt-in.
- Envoy closes TCP connections on a listener filter-chain change through drain,
  as a consequence of its filter-chain object model rather than as a revocation
  feature; host-set or health based connection closing is opt-in
  (`close_connections_on_host_set_change`, `close_connections_on_host_health_failure`).

Treating established flows as stable avoids resetting long-lived connections
on unrelated configuration churn, keeps the relay free of generation tracking,
and is consistent with the TCP/TLS timeout rule that an in-flight connection
never switches timeout generations and that in-flight relay updates are
intentionally absent
([L4 timeout snapshot publication](../../listeners/regression-guards/l4-timeout-snapshot-publication.md)).

## Alternatives and limits

Cost: revoking L4 access through configuration is not immediate for existing
flows. Operators who need immediate cut-off must also terminate the client or
backend connection (for example restart the backend pods, or rely on a shorter
`tcpTimeout.idleTimeout`). Active UDP clients keep their session until 60 s of
silence unless their pinned endpoint is removed.

Rejected for now: per-route-unit cancellation tokens or generation watches that
drain relays and UDP sessions on route removal, parent or ReferenceGrant
withdrawal, or stream-plugin scope change, with an optional `drainTimeout`. This
would add a cross-module revocation contract (route managers, plugin
publication, four L4 data paths, possibly a new EGC field) without a concrete
requirement. TCP/TLS endpoint removal is not acted on either: when the backend
process goes away the connection resets or idles out, but when an endpoint is
withdrawn from the Service while its pod stays alive (selector or label change,
slice withdrawal), the established relay keeps using that out-of-rotation pod.
UDP differs because a removed endpoint retires the session; opt-in TCP/TLS
host-removal closing (Envoy `close_connections_on_host_set_change`) is not
implemented.

UDP listener throughput is a separate, deferred concern. Each listener awaits
new-session admission inline. See the
[listener review](../../listeners/intentional-design/udp-session-table-spoofed-source-limit.md#resolution-references)
for the condition for revisiting receive-loop isolation.

## Verification and re-evaluation

No code change backs this decision; it rests on the source references above.
Re-evaluate when a security requirement demands immediate L4 revocation on route,
ReferenceGrant or policy change, when a drain/cancellation contract is introduced
for listener or route updates, or when TCP/TLS opt-in host-removal closing is
requested.

## Resolution references

- gw-core-05 (improve review, P2): configuration-change part accepted as designed
  2026-11-06; endpoint-removal part fixed for UDP; throughput part merged into
  gw-core3-05.
