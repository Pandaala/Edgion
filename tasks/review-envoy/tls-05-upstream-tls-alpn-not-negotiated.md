---
module: tls-secrets
title: Upstream TLS connections advertise only http/1.1 ALPN by default; gRPC-over-TLS backends requiring h2 ALPN receive the wrong protocol preference
severity: medium
envoy_reference: Envoy `UpstreamTlsContext.common_tls_context.alpn_protocols` — Envoy explicitly sets ALPN on every upstream TLS connection; for gRPC clusters this is always `["h2"]`; for HTTP/1.1 backends it is `["http/1.1"]`
---

# Upstream TLS connections advertise http/1.1 ALPN instead of h2 for gRPC backends

## Verification note

The original finding claimed "no ALPN extension is sent." This is factually
incorrect. Pingora's boringssl connector always sends ALPN when
`peer.get_alpn()` returns `Some` — and `PeerOptions::default()` initializes
`alpn: ALPN::H1` (pingora-core-0.8.0 `upstreams/peer.rs:484`). The connector
at `connectors/tls/boringssl_openssl/mod.rs:244-245` calls
`ssl_conf.set_alpn_protos(alpn.to_wire_preference())` unconditionally when
`alpn` is present. ALPN **is** sent — it just defaults to `http/1.1`.

For H2c (plaintext) backends `apply_backend_app_protocol` calls
`peer.options.set_http_version(2, 2)`, which sets `ALPN::H2` (per
`ALPN::new(2, 2)` in `protocols/tls/mod.rs:128-136`), so those backends
correctly receive `h2` ALPN. The gap is only for **TLS** backends that require
`h2`-only ALPN (i.e., gRPC-over-TLS), for which no equivalent path exists in
`create_tls_peer`.

The severity is downgraded from high to medium: the failure only occurs for
strict gRPC-over-TLS backends that require `h2` ALPN; HTTP/1.1 and H2c
backends are unaffected.

## Envoy's design (the proven baseline)

When Envoy opens a TLS connection to a backend it always sends the
`application_layer_protocol_negotiation` extension in the ClientHello, populated
from `UpstreamTlsContext.common_tls_context.alpn_protocols`. For gRPC clusters
this is hard-coded to `["h2"]`. The backend's TLS stack rejects or accepts the
connection based on ALPN, so a backend that requires gRPC/h2 correctly rejects
an HTTP/1.1 request at the TLS layer instead of letting a protocol mismatch
corrupt the stream.

## Edgion's current design

Upstream TLS connections are built by `create_tls_peer` in
`src/core/gateway/backends/peer_builder.rs:135-223`. This function calls:

- `HttpPeer::new(addr, true, sni)` — a Pingora standard TLS peer.
- `HttpPeer::new_mtls(addr, sni, cert_key)` — a Pingora mTLS peer.

Pingora's `PeerOptions::default()` initializes `alpn: ALPN::H1`, so these peers
advertise `http/1.1` ALPN in the ClientHello. `create_tls_peer` never overrides
`peer.options.alpn` for TLS gRPC backends.

The backend app protocol detection (`detect_backend_app_protocol_key` in
`peer_builder.rs:26`) reads the K8s Service `appProtocol` field and returns
`BackendAppProtocol::H2c` for `kubernetes.io/h2c` / `h2c`. `H2c` is plaintext
HTTP/2; `apply_backend_app_protocol` sets `peer.options.set_http_version(2, 2)`
which sets `ALPN::H2` correctly — but this path is only taken for plaintext
backends and is irrelevant to TLS.

For a TLS backend using gRPC (which requires h2 over TLS), there is no
corresponding path that sets `peer.options.alpn = ALPN::H2`.

`BackendTLSPolicy` (`peer_builder.rs:188-220`) carries `subjectAltNames` for
SAN validation but has no `alpn_protocols` field.

## The problem / gap

A gRPC backend that requires TLS + h2-only ALPN will:

1. Receive a ClientHello from Edgion advertising `http/1.1` ALPN.
2. If the backend is strict (e.g., a gRPC-Go server with `grpc.Creds` +
   `h2` required), it will either reject the connection at the TLS layer
   or complete the TLS handshake and then reject the HTTP/2 connection
   attempt, producing a cryptic error.
3. If the backend is lenient, it may negotiate without enforcing ALPN and
   serve h2 regardless — but this is implementation-defined behavior.

Additionally, when the proxy serves both HTTP/1.1 and HTTP/2 routes to the same
TLS backend and needs to distinguish them at the TLS layer (ALPN-based protocol
selection, RFC 7301), the default `http/1.1` ALPN prevents h2 negotiation even
when the backend supports it.

Envoy's explicit `alpn_protocols: ["h2"]` on gRPC clusters prevents the
ambiguity entirely.

## Impact

- Strict gRPC-over-TLS backends (those that require `h2` ALPN) may refuse
  connections from Edgion or silently fall back to HTTP/1.1.
- The failure is non-obvious: it appears as a TLS handshake error or a
  protocol negotiation failure, not as a routing misconfiguration.
- There is no way to configure ALPN for a specific TLS backend today — even if
  the operator knows the problem, they have no knob to fix it without code
  changes.

## Suggested direction

- Add `alpn_protocols: Option<Vec<String>>` to `BackendTLSPolicy.spec`. The
  controller resolves it; `create_tls_peer` passes the value to
  `peer.options.alpn` (using `ALPN::H2`, `ALPN::H1`, or `ALPN::H2H1`) before
  returning the peer.
- As a short-term heuristic: when `BackendTLSPolicy` is present and the route
  is a GRPCRoute, set `peer.options.alpn = ALPN::H2` in `create_tls_peer`,
  mirroring Envoy's auto-inference for gRPC clusters.
- For HTTP/1.1 TLS backends the current default `ALPN::H1` is correct; only
  gRPC-over-TLS backends need the change.
