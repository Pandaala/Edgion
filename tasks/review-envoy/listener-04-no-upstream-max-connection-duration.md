---
module: listener-upstream
title: No upstream max-connection-duration; long-lived H2 connections are never rotated
severity: medium
envoy_reference: Envoy `common_http_protocol_options.max_connection_duration` — closes keepalive connections after a wall-clock limit to rebalance across backends after scaling events
---

# No upstream max-connection-duration; long-lived H2 connections are never rotated

## Envoy's design (the proven baseline)

Envoy supports `max_connection_duration` on upstream clusters. When a keepalive
connection exceeds this age (regardless of activity), Envoy sends a HTTP/2 GOAWAY
(or closes the H1 connection after the current request) and opens a new connection.
This serves two goals:
1. **Rebalancing after scale-out**: when new backend pods start, existing keepalive
   connections continue to route to old pods. Without rotation, the new pods receive
   zero traffic until the old connections are closed by idle timeout or error.
2. **Cryptographic hygiene**: long-lived TLS connections accumulate key material age;
   periodic renegotiation limits exposure window.

For HTTP/1.1, `max_requests_per_connection` provides the same rotation signal by
request count rather than wall-clock duration.

## Edgion's current design

Edgion's `ParsedBackendTimeout` defines three timeout fields:
- `connect_timeout` (default 5s) — applied to connection establishment.
- `request_timeout` (default 60s) — applied to individual request I/O.
- `idle_timeout` (default 300s) — the connection is returned to the pool if idle
  for this duration (`edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs:667`,
  `edgion-gateway/src/routes/http/proxy_http/parse_timeout.rs:23`).

There is no `max_connection_duration` or `max_requests_per_connection` for upstream
connections. Once established, an upstream keepalive connection stays in the pool for
up to 300 seconds of idle time, with no upper bound on how long it has been alive
or how many requests it has served (beyond Pingora's own internal pool mechanics).

The downstream keepalive request limit (`downstream_keepalive_request_limit`,
`edgion-gateway/src/runtime/server/listener_builder.rs:199-216`) limits requests per
*downstream* connection but has no upstream analogue.

## The problem / gap

**Rebalancing gap**: After a Kubernetes `scale up` event, new backend pods start
receiving health-check traffic and are added to the EndpointSlice. The LB selectors
see the new endpoints. However, all currently active Pingora HTTP/2 keepalive
connections (which multiplex many requests) continue pointing to the old pods.
New pods only get traffic when old connections close. With a 300-second idle timeout
and active gRPC services that never go idle, old pods could remain overloaded for
the entire idle-timeout window (up to 5 minutes) while new pods are underutilized.

**H2 stream multiplexing makes this worse**: A single H2 connection carries many
concurrent gRPC streams. If that one connection stays pinned to an old pod, all
those streams go to the same pod regardless of LB selection, defeating least-conn
and EWMA algorithms entirely for the duration.

**No operator lever**: There is no configuration field to tune or disable this
behavior; operators who observe uneven load after scale-out have no knob to turn
without restarting the gateway.

## Impact

- Load imbalance after pod scale-out events; new pods are starved until idle timeout
  expires or connections error out.
- gRPC/H2 workloads are most affected because a single multiplexed connection
  bypasses the LB for all concurrent streams.
- EWMA and LeastConn LB strategies accumulate stale metrics from old pods and
  cannot self-correct while those connections are held open.

## Suggested direction

Add an optional `max_upstream_connection_duration` field to `BackendTimeout` in
`EdgionGatewayConfigSpec`. When set, track connection creation time (e.g., store
timestamp in Pingora's peer extension or in a wrapper) and refuse to reuse a
connection older than the limit, forcing the pool to open a fresh one.

For HTTP/2, coordinate with Pingora's H2 layer to send GOAWAY instead of abruptly
closing, so in-flight streams complete before the connection is retired.

A practical default of 3-5 minutes (shorter than the idle timeout) would be a safe
starting point for most deployments.
