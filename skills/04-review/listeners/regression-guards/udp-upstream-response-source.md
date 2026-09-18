---
name: udp-upstream-response-source
description: Use when reviewing UDP session upstream recv_from forwarding that ignores the datagram source, connected vs unconnected upstream sockets, or IPv4-mapped IPv6 comparison against a pinned backend IP and port.
knowledge_type: regression-guards
---

# UDP upstream response source must match the pinned backend

## Conclusion and scope

`EdgionUdpProxy` pins each UDP client session to one backend
`ClientSession.upstream_addr` (IP and port) at session creation. Upstream
datagrams are forwarded downstream only when the `recv_from` source matches
that pin after `IpAddr::to_canonical()` (IPv4-mapped IPv6 collapses to native
IPv4). Wrong IP and wrong port fail independently. Rejected datagrams are
dropped without incrementing `packets_received` / `bytes_received` and without
refreshing `last_activity`.

This is a receive-time check on the unconnected, family-matched upstream
socket. It does not `connect()` the socket. Destination safety checks
(`validate_endpoint_backend_addr`) and the IPv4/IPv6 bind-family helper stay
unchanged. Deprecated IPv4-compatible addresses (`::a.b.c.d`) are not treated
as mapped IPv4.

## Rationale and evidence

Source task `validate-udp-upstream-response-source` found that
`get_or_create_session` bound a family-matched ephemeral socket, sent with
`send_to` to the pinned endpoint, then discarded `recv_from`'s source and
forwarded the bytes. Any sender that could reach that ephemeral port could
supply forwarded payload. Reachability, port discovery, and application-level
validation constrain exploitation; no network exploit was claimed.

Current path:

- `edgion-gateway/src/routes/udp/edgion_udp.rs` `is_pinned_upstream_source`
  compares canonical IP plus port.
- `handle_upstream_packets` continues on a failed match before
  `record_downstream_send_result`.

Receive-time validation was chosen over a connected UDP socket so wrong-IP and
wrong-port cases remain observable in userspace tests, and so ICMP/`connect`
error behavior on the receiver loop does not change. Default-run coverage is
the pure `is_pinned_upstream_source` predicate; loopback `send_to`/`recv_from`
wire coverage is deferred because unit tests must not hit the network.

## Alternatives and limits

- Trigger: an established UDP session receives a datagram on its upstream
  socket whose source IP or port is not the pinned backend.
- Root cause: `recv_from` source was bound to `_` and forwarded unconditionally.
- Fixed behavior: only pinned sources are forwarded and accounted.
- Verification is crate unit tests of `is_pinned_upstream_source` (exact match,
  wrong IP, wrong port, IPv4-mapped accept/reject, native IPv6, IPv4-compatible
  non-match). Existing `edgion-tests` integration scenarios were left frozen;
  full socket/wire coverage is a separate authorized task if needed.
- Bind-family IPv6 defects remain covered by
  [udp-F017](../../backends/regression-guards/session-bind-address-family.md).
  Session destination pinning remains covered by
  [F-016](../../cpu-memory/regression-guards/udp-established-session-route-pipeline-per-packet.md).

## Verification and re-evaluation

Fresh verification in this fix:

- `cargo test -p edgion-gateway --lib routes::udp::edgion_udp`: 16 passed,
  0 failed.

Re-evaluate if:

- The upstream socket is `connect()`ed and kernel filtering replaces the
  userspace check.
- Dual-stack bind policy changes so IPv4-mapped sources appear on IPv6
  sockets more often, or IPv4-compatible addresses must be accepted.
- A requirement appears to accept a set of backends rather than one pinned
  IP:port per session.

## Resolution references

- `validate-udp-upstream-response-source`: confirmed defect, fixed with
  receive-time pinned IP/port validation.
