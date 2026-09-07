---
name: no-upstream-max-connection-duration-pingora-blocked
description: Use when reviewing findings that compare Edgion against Envoy and claim "no upstream max_connection_duration / max_requests_per_connection / long-lived H2 keepalive connections are never rotated / new pods starve after scale-out because old connections stay pinned / no GOAWAY-based upstream rebalancing". Covers why upstream connection-age rotation is a Pingora-pool-internal, upstream-API-blocked feature, not a narrow review fix.
status: accepted-tradeoff
finding_id: listener-04
closed: 2026-06-14
knowledge_type: compatibility-constraints
---

# No upstream max-connection-duration; long-lived H2 connections are never rotated — DESIGN-TRADEOFF (PINGORA-API-BLOCKED)

## Conclusion

Edgion exposes three upstream timeouts (`connect_timeout` 5s, `request_timeout` 60s,
`idle_timeout` 300s via `ParsedBackendTimeout`,
`edgion-gateway/src/routes/http/proxy_http/parse_timeout.rs:20-24`, applied at
`pg_upstream_peer.rs:667`) but has **no** upstream "max connection duration" or
"max requests per connection" rotation, the Envoy
`common_http_protocol_options.max_connection_duration` / `max_requests_per_connection`
analogue. A persistently-busy H2/gRPC keepalive connection therefore never rotates and can
keep multiplexed streams pinned to a pre-scale-out pod for up to the idle-timeout window.

This is a real feature gap, but the only correct fix is **blocked by the Pingora upstream
API** (the connection pool owns reuse), so it is an accepted, documented tradeoff — not a
narrow review fix. Fix suggestions of the form "add a `max_upstream_connection_duration`
field and refuse to reuse a connection older than the limit / store a creation timestamp in
the peer extension / send an upstream GOAWAY to retire the connection" are **not accepted**
under the review-issue flow; the work is a large cross-layer feature that belongs in
`tasks/todo/` via brainstorming (and is gated on upstream API support).

## Core Rationale

**1. The reuse decision lives inside Pingora's pool — Edgion cannot intercept it**

Among Pingora 0.8.1 `PeerOptions` (`pingora-core/src/upstreams/peer.rs:409`), the only
connection-lifecycle knobs are `connection_timeout`, `total_connection_timeout`,
`read_timeout`, `write_timeout`, `idle_timeout`, and `h2_ping_interval` / `max_h2_streams`.
There is **no** `max_connection_duration` and **no** `max_requests_per_connection`. The pool's
reuse-or-open-fresh decision is internal to Pingora; Edgion sets `idle_timeout` and then
hands the connection to the connector — there is no per-request hook where the proxy can say
"this pooled connection is too old, do not reuse it". `ProxyHttp::connected_to_upstream`
(`pingora-proxy/src/proxy_trait.rs:553`) is a post-connect **notification**, not a
reuse-rejection point. The `connection_filter` feature Edgion enables is **downstream**
pre-TLS L4 filtering (`pingora-core/src/listeners/connection_filter.rs`), unrelated to
upstream pooling.

**2. There is no API to retire an upstream H2 connection gracefully**

The issue's own "send a GOAWAY so in-flight streams complete before retirement" direction
requires control over Pingora's H2 upstream session lifecycle, which Pingora does not expose.
Without it, the only lever would be abruptly dropping connections, which defeats the purpose
(it would cancel in-flight streams). Implementing graceful rotation means reaching into or
forking the connection pool — exactly the wide-impact, upstream-gated work this flow excludes.

**3. The harm is bounded and self-healing; same class as the listener-rebind limitation**

The imbalance is transient: it manifests only at scale-out events with persistently-busy
H2/gRPC connections, and self-corrects once a connection idles past `idle_timeout`
(≤ 300s by default) or errors. No data loss, no crash. This mirrors
[listener-port-set-frozen-at-startup.md](../../listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md) (runtime
listener rebinding) and [gateway-no-local-config-snapshot.md](../../conf-sync/intentional-design/gateway-no-local-config-snapshot.md):
a genuine Envoy-parity gap whose fix is blocked on the serving framework, recorded as a
documented limitation rather than forced into a narrow patch. An operator who needs faster
rebalancing today can lower `default_idle_timeout`.

## Fix Suggestions Not Accepted

- "Add a `max_upstream_connection_duration` field to `BackendTimeout` and refuse to reuse an
  older connection" — there is no Edgion-side interception point for pool reuse; the field
  would be inert without forking Pingora's connector. Cross-layer feature, not a review fix.
- "Store the connection creation time in a Pingora peer extension / wrapper" — the wrapper
  cannot influence the pool's internal reuse selection in Pingora 0.8; it would track an age
  it cannot act on.
- "Coordinate with Pingora's H2 layer to send GOAWAY on retirement" — no such API is exposed;
  blocked upstream.
- "Add `max_requests_per_connection` as the H1 analogue" — same pool-internal limitation as
  the duration field.

## Re-evaluation Triggers

Re-open this decision only if:

- Pingora exposes an upstream connection max-age / max-reuse-count option on `PeerOptions`,
  or a pool hook that lets the proxy reject reuse of a connection by age/request-count (then
  design the `BackendTimeout` field + GOAWAY path via brainstorming, promote to `tasks/todo/`).
- A real production incident shows the post-scale-out imbalance persisting well beyond the
  idle-timeout window (i.e. the self-heal does not occur), elevating priority enough to
  justify a custom connector.

## Reference Cases

- `tasks/review-envoy/listener-04-no-upstream-max-connection-duration.md` (closed 2026-06-14, ⚠️ accepted tradeoff)
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs` (`:667`, upstream `idle_timeout` application)
- `edgion-gateway/src/routes/http/proxy_http/parse_timeout.rs` (`ParsedBackendTimeout`, the three shipping upstream timeouts)
- `pingora-core/src/upstreams/peer.rs` (`PeerOptions`, no max-connection-duration field)
- `skills/04-review/listeners/compatibility-constraints/listener-port-set-frozen-at-startup.md` (sibling upstream-API-blocked limitation)
- `skills/04-review/conf-sync/intentional-design/gateway-no-local-config-snapshot.md` (sibling accepted Envoy-parity gap)
