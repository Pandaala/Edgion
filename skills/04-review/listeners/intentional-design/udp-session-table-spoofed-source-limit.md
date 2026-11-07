---
name: udp-session-table-spoofed-source-limit
description: Use when reviewing UDP session admission, MAX_UDP_SESSIONS, SESSION_TIMEOUT, or claims that spoofed UDP sources can exhaust the per-listener session table.
knowledge_type: intentional-design
---

# UDP session table does not defend against spoofed sources

## Conclusion and scope

`EdgionUdpProxy` (`edgion-gateway/src/routes/udp/edgion_udp.rs`) admits one
session per direct peer address up to the fixed `MAX_UDP_SESSIONS` (10 000 per
listener), keeps it for the fixed `SESSION_TIMEOUT` (60 s idle), and drops new
datagrams with `SessionLimitReached` when the table is full. A sender that
forges many source addresses can therefore keep the table full and deny new
clients of that listener. This is an accepted L4 limitation: the gateway does
not try to distinguish forged sources, evict sessions under pressure, or
shorten the lifetime of sessions without a backend reply.

The decision covers only spoofed-source admission. Configurability of the cap
and timeout, the per-session footprint (dedicated socket, 64 KiB receive
buffer, 1 s receive poll), and receive-loop head-of-line latency are not
accepted; they remain open work in
[`tasks/todo/gw-core3-05-udp-session-admission-hardening.md`](../../../../tasks/todo/gw-core3-05-udp-session-admission-hardening.md).

## Rationale and evidence

- A UDP proxy cannot prove that a source address is genuine. The backend
  replies to the gateway's own upstream socket, not to the client, so a
  "backend replied" signal (conntrack `SEEN_REPLY` style) is reached by forged
  sessions whenever the backend answers the payload. Request/response services
  such as DNS answer valid forged queries, so a two-tier replied/unreplied
  timeout or unreplied-first eviction is defeated by sending valid payloads at
  the same cost. Requiring a client packet after the reply is also forgeable
  blindly.
- Per-source quotas key on the forged address and do not bound a flood that
  rotates sources.
- Envoy `udp_proxy` has the same shape: sessions keyed by the downstream
  4-tuple with their own upstream socket, a single `idle_timeout` (default
  1 minute), a cluster `max_connections` circuit breaker (default 1024) that
  drops new datagrams (`upstream_cx_overflow`), no replied/unreplied
  distinction, and no eviction. This is not a compatibility requirement, only
  evidence that mainstream L4 proxies leave spoofing defense to the network.
- Source-address spoofing is addressed at the network layer: ingress source
  filtering (BCP38) and provider DDoS protection in front of the listener.

## Alternatives and limits

Rejected for this finding:

- Two-tier lifetime with unreplied-first eviction (conntrack style):
  defeated by valid request payloads, as above.
- Per-source or per-prefix quotas: ineffective against rotating forged
  sources; may still be useful against a single misbehaving real client and
  is kept as an optional item in the task.
- Evicting the most idle session when full: admits new clients but drops
  low-rate legitimate clients under a strong flood, and requires a hot-path
  eviction order plus a change to the permit-ownership contract in
  [UDP session retirement](../regression-guards/udp-session-generation-retirement.md).
  Left as an open question in the task.

Accepted cost: a low-rate spoofed flood (about 10 000 new sources per 60 s)
can deny service to new clients of every UDPRoute on the listener, and the
gateway holds up to 10 000 sockets, tasks and receive buffers per listener
while it lasts. Established sessions keep forwarding, subject to the
single-serve-task head-of-line latency tracked in the task. When the backend
answers forged requests, the gateway sends those replies to the forged source
address, so a UDP listener in front of a request/response service can act as a
reflector; this is the same network-layer spoofing problem and is mitigated
the same way.

## Verification and re-evaluation

No code was changed. Evidence is a source review of `edgion_udp.rs`
(`get_or_create_session`, `handle_upstream_packets`,
`record_upstream_send_result`, `collect_retirement_candidates`) on
`feature-10-28` (2026-11-07) and the Envoy UDP proxy documentation. No load
test was run.

Re-evaluate if:

- the UDP cap or timeout becomes configurable or the per-session footprint
  changes (update this page's cost statement);
- a UDP listener gains a stateless/per-packet mode, a source-validation
  mechanism (for example a protocol-specific cookie), or a Stage-1 filter;
- a deployment target without network-layer anti-spoofing protection is
  declared supported;
- Envoy or another mainstream proxy adopts an effective in-proxy defense.

## Resolution references

- `gw-core3-05` (P2, CONFIRMED, re-verified STILL-VALID on `feature-10-28`):
  spoofed-source admission accepted on 2026-11-07; remaining hardening deferred
  to the task above.
- `gw-core-05` option 3 (single serve task head-of-line latency), merged into
  `gw-core3-05` on 2026-11-06: deferred to the same task.
