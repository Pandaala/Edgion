---
name: upstream-grpc-tls-alpn-set-by-grpc-peer-path
description: Use when reviewing upstream TLS ALPN findings claiming gRPC-over-TLS backends get http/1.1 ALPN because create_tls_peer never sets ALPN::H2; the gRPC peer path already forces h2. Also rejects "add an alpn_protocols CRD field" as a narrow review fix.
status: accepted-tradeoff
finding_id: tls-05
closed: 2026-06-14
---

# tls-05 — Upstream TLS ALPN for gRPC backends is set by the gRPC peer path, not `create_tls_peer` — FALSE-POSITIVE (primary) + ACCEPTED-TRADEOFF (residual)

## Conclusion

The finding's core claim — "no path sets `peer.options.alpn = ALPN::H2` for TLS gRPC
backends, so gRPC-over-TLS advertises `http/1.1` ALPN" — is **false**. gRPC traffic is
routed through `upstream_peer_grpc`, which forces `set_http_version(2, 2)`
(= `ALPN::H2`) on the peer **after** `create_tls_peer` returns. So gRPC-over-TLS
backends already receive `h2` ALPN, matching Envoy's `alpn_protocols: ["h2"]` on gRPC
clusters.

The only residual gap is a narrow, low-value one: a **non-gRPC HTTP/2-over-TLS backend
reached via HTTPRoute** stays at `ALPN::H1`, and there is no operator knob to set ALPN
for an arbitrary TLS backend. This is **accepted as a tradeoff** — it matches Envoy's
behavior without explicit `http2_protocol_options`, and adding the suggested
`alpn_protocols` CRD field is a wide-impact feature, not a narrow review fix.

Re-reports of the form "upstream TLS defaults to `http/1.1` ALPN", "gRPC-over-TLS gets
the wrong protocol preference", "`create_tls_peer` never overrides `peer.options.alpn`",
or "add `alpn_protocols: Option<Vec<String>>` to `BackendTLSPolicy.spec`" are **not
accepted**.

## Core Rationale

**1. The gRPC peer path forces h2 ALPN, above `create_tls_peer`**

`create_tls_peer` (`edgion-gateway/src/backends/peer_builder.rs:135-219`) indeed never
touches `peer.options.alpn`, so a TLS peer leaves it at Pingora's
`PeerOptions::default()` of `ALPN::H1` (`pingora-core-0.8.1 upstreams/peer.rs:484`). The
finding's verification note inspected only `create_tls_peer` and
`apply_backend_app_protocol` and concluded no h2 path exists. It missed the layer that
actually handles gRPC:

- A matched GRPCRoute sets `ctx.is_grpc_route_matched = true`
  (`edgion-gateway/src/routes/grpc/integration.rs:58`).
- `upstream_peer` routes on that flag into `upstream_peer_grpc`
  (`edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:183`).
- `upstream_peer_grpc` calls `get_peer` (which calls `create_tls_peer`) and then, at
  `pg_upstream_peer.rs:226-227`, unconditionally runs `peer.options.set_http_version(2, 2)`.
- `set_http_version(2, 2)` sets `self.alpn = ALPN::new(2, 2)` = `ALPN::H2`
  (`pingora-core-0.8.1 upstreams/peer.rs:512-513`).

Because this runs after `create_tls_peer`, the H1 default is overwritten before the peer
is used, so the ClientHello advertises `h2`. This is the same `set_http_version(2, 2)`
mechanism that makes the H2c plaintext path correct — gRPC-over-TLS just gets it one
layer up, in `upstream_peer_grpc`, rather than inside `create_tls_peer`.

**2. The residual non-gRPC h2-over-TLS gap is at the Envoy baseline**

The only case left at `ALPN::H1` is a backend that speaks HTTP/2 over TLS but is matched
by an HTTPRoute (not a GRPCRoute, not the `h2c` plaintext `appProtocol`). Dispatch keys
off the matched route kind, not the wire protocol — so even a gRPC payload that happens
to match an HTTPRoute over TLS falls in this bucket. Stock Envoy also
keeps upstream HTTP/1.1 unless `http2_protocol_options` is explicitly configured on the
cluster, and Kubernetes `appProtocol` has no standard "h2 over TLS" value (`kubernetes.io/h2c`
is cleartext). So Edgion is not below the proven baseline for this exotic combination.

**3. The suggested fix is a wide-impact feature, not a narrow review fix**

Adding `alpn_protocols: Option<Vec<String>>` to `BackendTLSPolicy.spec` is a CRD schema
change that must be threaded through `edgion-resources` (schema) → controller (resolve)
→ gateway (`create_tls_peer` apply). That is a cross-layer feature with its own
compatibility and validation surface — it belongs in `tasks/todo/` via brainstorming if
ever prioritized, not in the narrow `/fix-issue` workflow.

## Fix Suggestions Not Accepted

- "Add `alpn_protocols: Option<Vec<String>>` to `BackendTLSPolicy.spec` and apply it in
  `create_tls_peer`" — wide-impact CRD/cross-layer feature; the real gRPC case already
  works, so the value does not justify the blast radius. Promote to `tasks/todo/` if a
  concrete non-gRPC h2-over-TLS need appears.
- "Short-term heuristic: set `peer.options.alpn = ALPN::H2` in `create_tls_peer` when the
  route is a GRPCRoute" — redundant. `upstream_peer_grpc` already forces h2 for every
  gRPC route; duplicating it inside `create_tls_peer` would be a second source of truth
  for the same decision.
- "Override `peer.options.alpn` unconditionally in `create_tls_peer`" — would break the
  correct `http/1.1` default for ordinary HTTPRoute-over-TLS backends.

## Re-evaluation Triggers

Re-open this decision only if:

- A real deployment needs h2 over TLS for a **non-gRPC** backend routed via HTTPRoute and
  has no plaintext `h2c` option — then the `alpn_protocols` field becomes a justified
  `tasks/todo/` feature.
- The gRPC peer path stops forcing `set_http_version(2, 2)` (e.g. a refactor of
  `upstream_peer_grpc`) — then gRPC-over-TLS ALPN must be re-established, and
  `create_tls_peer` becomes the natural home for it.

## Reference Cases

- Issue: `tasks/review-envoy/tls-05-upstream-tls-alpn-not-negotiated.md` (closed 2026-06-14)
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:183,226-227`
  (`upstream_peer_grpc` forces h2)
- `edgion-gateway/src/routes/grpc/integration.rs:58` (`is_grpc_route_matched = true`)
- `edgion-gateway/src/backends/peer_builder.rs:135-219` (`create_tls_peer`)
- Envoy: `UpstreamTlsContext.common_tls_context.alpn_protocols` / cluster
  `http2_protocol_options` (explicit-opt-in baseline)
