---
module: listener-upstream
title: Connect timeout for TCP/TLS L4 routes is not independently configurable; no upstream connect timeout metric
severity: low
envoy_reference: Envoy cluster `connect_timeout` is separate from `request_timeout` and surfaced in `upstream_cx_connect_timeout_total` metric
---

# Connect timeout for TCP/TLS L4 routes is not independently configurable; no upstream connect timeout metric

## Envoy's design (the proven baseline)

Envoy separates connection establishment timeout from request/stream timeout at
cluster level. `connect_timeout` applies only to TCP handshake (plus TLS handshake
if applicable); it is independently configurable and monitored via the
`upstream_cx_connect_timeout_total` counter per cluster. This makes it easy to
distinguish "backend is reachable but slow" (request timeout) from "backend is
not reachable" (connect timeout) in monitoring.

## Edgion's current design

**HTTP/gRPC path**: Edgion correctly separates connection timeout from request
timeout. `configure_peer_timeouts()` sets `peer.options.connection_timeout` separately
from read/write timeouts (`edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:610-668`),
and the connect timeout is drawn from `BackendTimeout.default_connect_timeout`
(default 5s, configurable via `EdgionGatewayConfigSpec.http_timeout.backend`).
This path is well-designed.

**TCP/TLS L4 proxy path**: `EdgionTcpProxy` and `EdgionTlsTcpProxy` use
`TransportConnector::new(None)`, which accepts an optional `timeout` parameter.
Neither passes a connect timeout; the value comes from Pingora's internal default
(indefinite or OS TCP retransmit timeout if unset). Looking at the code in
`edgion-gateway/src/routes/tcp/edgion_tcp.rs:199` and
`edgion-gateway/src/routes/tls/proxy.rs:296`, there is no timeout set on
`connector.new_stream(&peer)`.

There is also no Prometheus counter for connect timeouts on TCP/TLS L4 routes
(the HTTP path's `ct` (connect time) is captured in `UpstreamInfo` but L4 routes
have no equivalent).

## The problem / gap

For TCP/TLS L4 routes:
- If a backend is unreachable (firewall drop, pod not yet ready), the L4 proxy
  will hang waiting for a TCP SYN-ACK with no upper bound, holding the downstream
  client connection open indefinitely.
- There is no annotation or CRD field to configure a TCP L4 connect timeout, unlike
  the rich `http_timeout.backend` section available for HTTP routes.
- There is no metric to distinguish connect failures from data-transfer failures on
  L4 routes; both appear as `UpstreamConnectionFailed` in the TCP access log with
  no timing breakdown.

## Impact

Lower severity than the other findings because:
- The OS TCP stack does eventually time out (default ~2 minutes on Linux), so this is
  not an infinite hang.
- TLS/TCP routes are typically internal cluster traffic where connect failures are
  rare and quickly self-heal.

However, in slow-fail scenarios (e.g., firewall-dropped SYNs), the L4 downstream
client holds a connection open for up to 2 minutes with no indication of progress,
burning a file descriptor and potentially leaving the client's own timeout unexpired
during that window.

## Suggested direction

Add an optional `connect_timeout` annotation (e.g., `edgion.io/connect-timeout`)
to TLSRoute and TCPRoute, or add a global `l4_timeout` section to
`EdgionGatewayConfigSpec` mirroring `http_timeout.backend.default_connect_timeout`.
Pass this value as the timeout argument to `TransportConnector::new(Some(timeout))`.

Add a Prometheus counter `edgion_l4_upstream_connect_timeout_total` with labels
`{protocol, service}` so operators can monitor TCP-level connection failures
separately from data-transfer errors.
