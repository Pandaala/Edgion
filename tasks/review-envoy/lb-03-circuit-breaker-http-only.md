---
module: load-balancing
title: Circuit breaker (outlier detection) is HTTP-only — TCP/UDP/TLS stream routes have no passive health signal
severity: high
envoy_reference: Envoy outlier detection applies to all upstream cluster types including TCP clusters
---

# Circuit Breaker Is HTTP-Only; TCP/UDP/TLS Stream Routes Have No Passive Health Feedback

## Envoy's design (the proven baseline)

Envoy's outlier detection is a cluster-level concept, not a protocol-level concept.
It intercepts connection outcomes (TCP connect success/failure) and, for HTTP
clusters, also inspects response status codes. A TCP cluster where backends
repeatedly fail to accept connections will eject those backends via consecutive
connect-failure tracking, exactly the same mechanism as HTTP 5xx ejection. This
means TCP, HTTP/1, HTTP/2, and gRPC clusters all benefit from passive health
feedback without requiring active health probes.

Reference: Envoy docs — Outlier Detection; consecutive gateway errors; TCP connect
failures counted as consecutive 5xx equivalents.

## Edgion's current design

Edgion's circuit-breaker feedback is recorded in exactly three places, all inside
the HTTP proxy path:

- `pg_fail_to_connect.rs:41` — `record_failure_by_addr` on upstream TCP connect
  failure (HTTP/gRPC proxy only).
- `pg_error_while_proxy.rs:33` — `record_failure_by_addr` on mid-stream error
  (HTTP proxy only, non-status-retry path).
- `pg_upstream_response_filter.rs:177,179` — `record_failure_by_addr` /
  `record_success_by_addr` on HTTP status >= 500 / < 500.

TCP routes (`edgion-gateway/src/routes/tcp/edgion_tcp.rs:159-178`) call
`select_roundrobin_backend_key` which honours the circuit breaker via
`is_backend_healthy` (`selector.rs:49-53`). However, when a TCP upstream connection
fails (`TcpStatus::UpstreamConnectionFailed`, `edgion_tcp.rs:199-204`), no
`record_failure_by_addr` is called. Likewise for UDP (`edgion_udp.rs`) and
TLS passthrough (`tls/passthrough_proxy.rs`).

The circuit-breaker store is configured per-service (`has_service` gate in
`store.rs:238`), so even if TCP routes were to call `record_failure_by_addr`, the
service would need to have a circuit-breaker annotation — the gate is present, the
call site is simply missing.

## The problem / gap

A TCP or TLS-passthrough backend that is repeatedly refusing connections — because
the pod is crash-looping or the port is closed — accumulates no circuit-breaker
state. Every new TCP connection from every new client will attempt to connect to
that backend until the active health check (if configured) marks it unhealthy, or
the operator intervenes. With a default active HC interval of 10 seconds and an
unhealthy threshold of 3, that is up to 30 seconds of repeated failed TCP connects
before the backend is filtered out.

For gRPC routes (which go through the HTTP proxy path) this works correctly.
For raw TCP/UDP/TLS-passthrough routes, there is a gap: the circuit-breaker ejection
check is consulted during backend selection (via `is_backend_healthy`) but is never
fed by the outcome of the connection attempt.

Additionally, `success` is never recorded for TCP routes, so a backend's
`consecutive_failures` counter (if it ever got incremented) could never be reset
via the passive path.

## Impact

- **No automatic ejection of failing TCP/UDP backends** without active health checks;
  every client absorbs the connection latency (TCP RST or timeout) for the full
  active-HC reaction period.
- **Inconsistent resilience model**: HTTP backends get passive ejection, TCP backends
  do not — operators may not realize TCP routes require active HC to be configured
  explicitly for equivalent protection.
- **Load concentration**: remaining healthy TCP backends absorb retried connections
  from all clients during the failure window, potentially causing cascading overload.

## Suggested direction

Add `record_failure_by_addr` and `record_success_by_addr` calls in the TCP, UDP,
and TLS-passthrough connection outcome paths, mirroring the HTTP path. For TCP:
after a failed `connector.new_stream`, call `record_failure_by_addr`; after a
successful relay session ends cleanly, call `record_success_by_addr`. The
circuit-breaker store's `has_service` guard already acts as a no-op gate when no
circuit-breaker is configured, so these calls are cheap for unconfigured services.
The same backend-key and service-key plumbing already present in the HTTP path
(via `upstream.service_key` and `upstream.lb_backend_addr`) must be threaded into
the TCP/UDP contexts.
