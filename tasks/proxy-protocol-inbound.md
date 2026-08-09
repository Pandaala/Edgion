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
endpoint IDs; note Istio's ambient mesh has claimed `0xD0`/`0xD1`), and
recovering the balancer's own address after the swap (nginx's
`$realip_remote_addr` analog — the fd still holds it if ever needed).

## Operational notes

- **Trust.** The header is unauthenticated; anything that can reach the listener
  directly can claim any source address. The listener must be reachable only
  through the balancer, enforced by security groups / private subnets. This
  matches Envoy, Istio, nginx, and HAProxy, none of which do CIDR checks by
  default. A trusted-CIDR allowlist (Traefik's model) is a natural later
  hardening step: the real fd peer is still available before the swap.
- **Health checks.** kubelet liveness/readiness probes hit the pod directly and
  carry no header, so never enable this on a listener that also serves probes.
  Edgion's admin/probe ports are separate listeners and are unaffected. Istio hit
  exactly this (istio/istio#39868) and scoped its exception to the status port
  rather than adding a global permissive mode.
- **Listener merging.** Listeners are deduplicated globally by
  `(ip:port, protocol)`, first one wins. Two Gateway listeners collapsing onto
  one physical port cannot disagree about the flag; whichever binds first
  decides. A controller-side consistency check in `listener_port_manager.rs`
  would be the place to reject that up front.
- **No hot rebind.** The flag is read once when listeners bind, so changing it
  requires a gateway restart.

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
