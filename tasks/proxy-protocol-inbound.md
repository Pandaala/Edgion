# Inbound PROXY protocol (v1/v2) support

Status: **implemented**

## Problem

A gateway behind an L4 load balancer (AWS NLB with proxy protocol v2, HAProxy,
an nginx stream tier) sees the balancer as its TCP peer. Every IP-based decision
downstream — access logs, `X-Forwarded-For` / `X-Real-IP` injection, IP
allow/deny plugins, per-client rate limits, DSL and Wasm client-IP builtins —
then keys off the wrong address. The standard fix is the HAProxy PROXY protocol:
the balancer prepends a small header carrying the original addresses before any
application bytes, including the TLS ClientHello.

Edgion already **sends** PPv2 upstream (`edgion.io/proxy-protocol` on
`TLSRoute`). This work covers the opposite direction: **accepting** a header on
inbound connections.

## Where the parsing lives

In the pingora fork (`/Users/caohao/ws2/pingora`, branch `edgion`), not in
Edgion — see `tasks/proxy-protocol.md` there. Summary of the contract Edgion
relies on:

- `TcpSocketOptions.proxy_protocol: bool` enables it per listening endpoint.
- The header is consumed at the top of `UninitializedStream::handshake()`, after
  `set_buffer()` and **before** the TLS handshake, so it works identically for
  HTTP, HTTPS, TCP, and TLS listeners.
- On success the stream's `SocketDigest` is replaced with one whose `peer_addr`
  is the PROXY source and `local_addr` the PROXY destination. Everything that
  reads the downstream address goes through that digest, so consumers need no
  changes.
- Mandatory mode: a listener with the flag on **rejects** any connection that
  does not start with a valid header (HAProxy `accept-proxy` semantics). There
  is deliberately no sniff-and-fall-back mode — that would let any direct client
  choose whether to be trusted.
- v1 and v2 are auto-detected. `LOCAL`, UNSPEC, and non-STREAM headers are
  consumed but keep the real socket addresses (the load-balancer health-check
  path).

## Config surface

`Gateway.metadata.annotations`:

| Key | Effect |
|-----|--------|
| `edgion.io/accept-proxy-protocol: "true"` | every listener of this Gateway expects a header |
| `edgion.io/accept-proxy-protocol.<listener-name>: "true"\|"false"` | overrides the Gateway-wide value for one listener |

### Why an annotation

The alternatives were each ruled out on evidence, not taste:

- **A `Listener` spec field is impossible.** Gateway API has no PROXY protocol
  field and will not get one: `kubernetes-sigs/gateway-api#1955` was accepted as
  a GEP candidate in 2023 and auto-closed `not_planned` in May 2025 without ever
  receiving a number. `grep -i proxy` over the experimental-channel Gateway CRD
  returns nothing, and ListenerSets (Standard in v1.5) added no per-listener
  extension point either. Edgion vendors the upstream CRD, which is a structural
  schema that silently prunes undeclared fields — the codebase already documents
  this as the reason knobs live in annotations.
- **`EdgionGatewayConfig` is the wrong granularity.** It attaches via
  `GatewayClass.parametersRef`, so a field there applies to every Gateway of the
  class. Whether a gateway sits behind an LB is a per-deployment fact.
- **`tls.options`** is genuinely per-listener but only exists on TLS listeners,
  so it cannot cover HTTP or TCP.

GEP-713 discourages annotations in favour of policy CRDs with
`targetRef.sectionName`, which is the only sanctioned per-listener knob. That
remains the migration target if Edgion ever grows a listener-policy CRD; for a
single boolean it is not worth a new CRD, a controller reconciler, and status
plumbing. Istio's own first-class API for this is a bare on/off
(`meshConfig.defaultConfig.gatewayTopology.proxyProtocol: {}`), which suggests a
boolean is the right shape.

Note a Gateway API `Listener` has no annotations of its own, which is why the
per-listener form is a key suffix rather than a nested field.

## Implementation

| File | Change |
|------|--------|
| `edgion-resources/src/constants/annotations.rs` | `ACCEPT_PROXY_PROTOCOL` constant + trust-model docs |
| `edgion-gateway/src/runtime/server/listener_builder.rs` | `accept_proxy_protocol()`, `tcp_sock_opts()`, `add_tcp_endpoint()`, `tcp_listeners()`; threaded into all six TCP/TLS bind sites; enablement log + stream-plugin warning |

No CRD, proto, or controller change: the annotation rides in `metadata.annotations`
of the JSON the controller already syncs, and `gateway_annotations` was already
threaded into `ListenerContext`.

`tcp_sock_opts()` returns `Option<TcpSocketOptions>` and yields `None` when the
flag is off, so a listener without it takes byte-for-byte the code path it took
before this feature existed. Cost when disabled is one map lookup per listener at
bind time and one bool check per accepted connection inside pingora.

`TcpSocketOptions` is `#[non_exhaustive]`, so it must be built by mutating a
`Default` — a struct expression is rejected across crates even with
`..Default::default()`.

## What this does and does not fix

Fixed automatically, no code change, because they all read the socket digest:
`X-Forwarded-For` and `X-Real-IP` injection, the RealIp extractor's trust anchor,
`ip_restriction` and `global_access_control` plugins, `KeyGet::ClientIp` rate
limiting, EdgionDSL `ReqClientIp` / `ReqRemoteIp`, the Wasm host's
`source.address`, link_sys templates, and the HTTP/TCP/TLS access logs.

**Not fixed — `edgion.io/edgion-stream-plugins`.** That bridges to pingora's
`ConnectionFilter`, which runs at accept time, before the header is parsed, so
its IP rules match the load balancer. The gateway logs a warning when both are
enabled on the same listener. Fixing it properly means moving the filter after
the handshake, which changes when connections are shed and is out of scope here.

Also out of scope: UDP listeners (no PROXY protocol), v2 TLV parsing (AWS VPC
endpoint IDs; note Istio's ambient mesh has claimed `0xD0`/`0xD1`), and preserving the original destination across hops (each hop rewrites
`local_addr` to its own upstream address). The balancer's own address is no
longer a non-goal: it is available as `SocketDigest::raw_peer_addr` and appears
in the L4 access logs as `transport_peer`.

## Operational notes

- **Trust.** The header is unauthenticated. Without
  `edgion.io/proxy-protocol-trusted-cidrs` the listener must be reachable only
  through the balancer, enforced by security groups / private subnets. With it,
  only peers in the list are examined and everything else is served as an
  ordinary direct connection — that is how one listener safely serves both
  load-balanced and direct traffic.
- **Health checks.** kubelet liveness/readiness probes hit the pod directly and
  carry no header. Under mandatory mode never enable this on a listener that
  also serves probes; under source-conditional mode the probe's address simply
  falls outside the trusted list and is served normally, which is the main
  reason to use that mode.
  Edgion's admin/probe ports are separate listeners and are unaffected. Istio hit
  exactly this (istio/istio#39868) and scoped its exception to the status port
  rather than adding a global permissive mode.
- **Listener merging.** Listeners are deduplicated globally by
  `(ip:port, protocol)`, first one wins. Two Gateway listeners collapsing onto
  one physical port cannot disagree about the flag *or the trusted-CIDR list*;
  whichever binds first decides both. A controller-side consistency check in `listener_port_manager.rs`
  would be the place to reject that up front.
- **No hot rebind.** The flag is read once when listeners bind, so changing it
  requires a gateway restart.

## Client address resolution

1. `raw_peer` — the transport peer. Always known; the load balancer on a
   proxy-protocol listener.
2. PROXY protocol (L4). If enabled, the peer is trusted and a header is present,
   `peer_addr` becomes the header's source. Otherwise `peer_addr` stays
   `raw_peer`.
3. X-Forwarded-For (L7), i.e. `realIp`. If `peer_addr` is in
   `realIp.trustedIps`, the XFF chain is walked right-to-left from it under
   `recursive` / `maxTrustedHops`. Otherwise `peer_addr` is the answer.

XFF wins when it is trustworthy, the PROXY address is the fallback, the
transport peer is the last resort. Every tier is gated, so a client cannot
promote itself: a forged XFF is only consulted when the peer is already a
trusted proxy, and a forged PROXY header is only parsed when the peer is in the
listener's trusted sources.

Note tier 3 is inactive in the plain load-balancer deployment. Once a header is
applied the anchor is the real client, who is not a trusted proxy, so the XFF
walk stops immediately. It matters in a gateway chain, where the PROXY source is
a peer gateway the operator has listed in `realIp.trustedIps`.

Two trust lists, never to be conflated:

- `edgion.io/proxy-protocol-trusted-cidrs` (L4) — who may SPEAK proxy protocol.
  Holds the load balancer.
- `EdgionGatewayConfig.spec.realIp.trustedIps` (L7) — which addresses count as
  proxy hops when walking XFF. Holds CDN egress ranges and peer gateways.

Forcing modes need no extra configuration: force-PROXY by keeping the PROXY
address out of `realIp.trustedIps`; force-XFF by not enabling proxy protocol on
that listener.

## Only reachable through something that writes the header

A PROXY-accepting listener must be reachable only via a component that writes
the header **itself**. A transparent L4 relay does not qualify: it forwards the
client's first bytes verbatim, so a client can author the header and the relay
launders it into a trusted position. This applies to mandatory mode as much as
to source-conditional mode, and equally to HAProxy and nginx relays — it is a
property of transparent relaying, not of this implementation.

Concretely, do not put a gateway whose TCPRoutes are plain relays in front of a
PROXY-accepting listener, and do not list such a gateway in
`proxy-protocol-trusted-cidrs`. HTTPRoute hops are safe (they reparse the
request) and TLSRoute hops are safe (they parse the ClientHello and can emit
their own header via `edgion.io/proxy-protocol: v2`).

## Known limitations

- **Source-conditional mode waits for 12 bytes.** Deciding whether a trusted
  peer sent a header requires reading the signature, so a trusted peer that
  connects and says nothing is held until the 60s downstream handshake timeout.
  Mandatory mode does not share this: there a silent peer is meant to be
  rejected. Do not use source-conditional mode on listeners fronting
  server-speaks-first protocols (SMTP, MySQL, Postgres).
- **One trust list per Gateway.** `accept-proxy-protocol` has a per-listener
  suffix; `proxy-protocol-trusted-cidrs` does not, so all PROXY-enabled
  listeners of a Gateway share one list.

## Testing

`edgion-tests/integration/scripts/integration/test_proxy_protocol_inbound.sh`,
standalone (a PP-enabled listener is unusable by other suites, and the flag
cannot be toggled without a restart). Fixtures in
`edgion-tests/integration/conf/ProxyProtocolInbound/`, ports 584xx.

Real proxies produce the headers rather than hand-assembled bytes — the byte
level is already unit-tested in the fork. **One HAProxy drives both legs**:
`send-proxy-v2` for v2, `send-proxy` for v1. It is the reference implementation
(the spec is a HAProxy document) and emits both versions from one binary, so the
two legs share sender, image, and topology and differ in exactly one variable —
which makes a failure on either attributable to the protocol version alone.

**nginx is deliberately not used.** It can only ever *send* v1 (it receives v2
but has never implemented emitting it; verified empirically against
`nginx:1.27-alpine`, which emits `PROXY TCP4 ... \r\n`), so it could not cover
the v2 leg, and mixing senders would make the legs differ by more than the
version. Operators putting nginx in front therefore get v1, which this test
covers via the equivalent HAProxy leg.

A client container pinned to `172.28.0.100` on a private docker network makes the
assertion unambiguous: the gateway can never see that address as a raw TCP peer,
since HAProxy reaches the host through NAT. Phases:

1. **Wire-version preflight.** Everything else asserts the same thing on both
   legs (the client IP), so a leg silently emitting the other version would still
   pass and leave us wrongly believing both parser paths were covered. Each leg
   has a twin listener in the same config pointed at a byte dumper on the host,
   so this observes the genuine emission: the v2 leg must lead with the signature
   `0d0a0d0a000d0a515549540a` followed by `0x21` (version 2 + PROXY command), the
   v1 leg with ASCII `PROXY `. Either leg emitting the other version fails, which
   is what catches `send-proxy` and `send-proxy-v2` being swapped.
2. via HAProxy `send-proxy-v2` → `X-Forwarded-For` / `X-Real-IP` == `172.28.0.100`
3. via HAProxy `send-proxy` (v1) → same
4. direct to the PP listener with no header → no HTTP response, and the gateway
   log shows a proxy protocol rejection (so the drop is attributed, not assumed)
5. direct to the `plain` listener → 200 with `X-Forwarded-For: 127.0.0.1`,
   proving the per-listener `"false"` override beats the Gateway-wide `"true"`

Startup also asserts the gateway logged the enablement line, so a silently
ignored annotation fails the test rather than passing vacuously.

### Two-hop chain suite

`edgion-tests/integration/scripts/integration/test_proxy_protocol_chain.sh`,
fixtures in `edgion-tests/integration/conf/ProxyProtocolChain/`, ports 585xx and
docker subnet `172.29.0.0/16` (the single-hop suite holds 584xx and
`172.28.0.0/16`; docker refuses overlapping pools, so the two must differ).

HAProxy sends PPv2 to hop 1, hop 1 routes to hop 2 over plain HTTP, hop 2 routes
to the test server. Hop 1 derives the client from the PROXY header and appends it
to X-Forwarded-For; hop 2 has no PROXY annotation — nothing sends it a header —
and resolves the client by walking XFF, which works only because the GatewayClass
carries a `parametersRef` to an `EdgionGatewayConfig` listing hop 1 in
`realIp.trustedIps`.

**The assertion is on X-Real-IP, not X-Forwarded-For.** XFF is appended
unconditionally by every hop, so it carries the client whether or not hop 2
resolved anything; asserting on it alone passes green while every IP-based plugin
at hop 2 still sees hop 1. X-Real-IP is written from the resolved address.
Confirmed by deleting `realIp` from the fixture: X-Real-IP degrades to
`127.0.0.1` and the suite fails, while both XFF assertions still pass.

Only the v2 leg is driven here — what this suite tests is propagation across
hops, not the parsers, which the single-hop suite covers for both versions.

Both suites are registered in `run_integration.sh` behind a docker-availability
guard, and both are excluded from the shared conf loader in
`start_all_with_conf.sh`.
