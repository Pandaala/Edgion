---
name: ws-01-03-handshake-not-validated
description: Accepted RFC 6455 §4.1/§4.2.1/§4.4 divergence — Edgion is a transparent WS tunnel and does not validate the handshake (Connection token, Sec-WebSocket-Version, GET method) nor synthesize 426/400; the terminating backend enforces. Reject re-reports proposing gateway-side handshake validation.
---

# ws-01 / ws-02 / ws-03 — WebSocket handshake not validated at the gateway

**Status:** ⚠️ accepted divergence · 2026-05-21
**RFC:** RFC 6455 §4.1, §4.2.1, §4.4 (MUST — but binds the handshake-completing server)

## Conclusion

Edgion forwards the WebSocket handshake transparently. It does not verify the
`Connection: Upgrade` token (ws-01), does not validate `Sec-WebSocket-Version: 13`
or respond with `426` (ws-02), and does not enforce the `GET` method (ws-03). The
`discover_protocol = "websocket"` tag is an observability/metadata label that
mirrors Pingora's own upgrade-tunnel trigger — it is **not** a handshake gate. The
endpoint that completes the handshake (sends `101 Switching Protocols`) is the
upstream backend, and the RFC's server-side MUSTs bind that endpoint.

Suggestions of the form "require the `Connection` token / assert `GET` /
return `426` + `Sec-WebSocket-Version: 13` at the gateway" are **not accepted** —
they pivot Edgion from a transparent tunnel into a terminating WS server.

## RFC clause

> "The server MUST verify ... that the request contains a |Connection| header
> field whose value includes the 'Upgrade' token." — §4.2.1 (ws-01)

> "If the server does not support the version ... it MUST respond with a
> |Sec-WebSocket-Version| header field ... containing all versions it is willing
> to use." — §4.4 (ws-02)

> "The method of the request MUST be GET." — §4.1 (ws-03)

## Code shape today

- `routes/http/proxy_http/pg_request_filter.rs:256-261` — WS is tagged purely from
  `Upgrade: websocket`; no read of `Connection`, `Sec-WebSocket-Version`, or method.
- The tag drives only observability (`pg_logging.rs` protocol field,
  `pg_connected_to_upstream.rs` conn-est entry), the DSL `req_protocol()` builtin,
  and the `appProtocol: ws` backend HTTP-version hint (`peer_builder.rs:53-56`).
  It does not gate the upgrade — Pingora establishes the tunnel from the request's
  `Upgrade` header, and the `101` comes from the upstream.

## Why we diverge

1. **The MUSTs bind the terminating server, which is the backend.** In Edgion's
   transparent-tunnel model the gateway never completes the handshake; tightening
   the tag would make the label *diverge* from the actual data-plane tunnel trigger
   (Pingora keys on `Upgrade` presence alone), and synthesizing `426`/`400` would
   make Edgion a partial WS terminator. Same architectural class as [[ws-04-origin-not-validated]].
2. **Industry norm is to delegate to the backend.** No major L7 gateway (Envoy,
   NGINX, Traefik, Caddy, HAProxy, Kong, APISIX) validates the WS handshake itself;
   all tunnel transparently and let the app enforce. Gateway API has no WS profile.
3. **Negligible harm.** All P3. Browsers always send `GET` + `Connection: Upgrade`
   + `Sec-WebSocket-Version: 13`, so only crafted/non-browser clients diverge, and
   the upstream rejects them anyway. The only residual cost is a slightly loose
   observability label, which already matches what Edgion actually does (tunnel).

## Re-evaluation triggers

- Gateway API introduces a WebSocket conformance profile / handshake-validation field.
- Edgion pivots from transparent WS tunnel to a terminating WS server (e.g., gains a
  WS-aware plugin stage that needs an authoritative handshake decision).
- An operator-reported incident where the loose `websocket` label causes a concrete
  routing/policy misfire that the backend cannot remediate.

## Reference

- Verifier: `tasks/todo/rfc-review/_raw/verification-a3.md → V1 (ws-01), V2 (ws-02), V3 (ws-03)`
- Related divergence: [[ws-04-origin-not-validated]] (same transparent-tunnel rationale, SHOULD/MAY)
- WS-over-H2 (RFC 8441) downgrade gap is tracked separately as ws-05.
- Closure: [CLOSED-FINDINGS.md](../CLOSED-FINDINGS.md)
