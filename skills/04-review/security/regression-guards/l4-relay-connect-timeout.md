---
name: l4-relay-connect-timeout
description: Use when reviewing L4 connect-timeout findings; covers the landed default connect timeout that bounds upstream connect on all three L4 proxies, and the rejected per-route-annotation + per-service-metric suggestions — do not re-report "TCP/TLS L4 connect is unbounded / not configurable".
status: fixed
finding_id: listener-06
closed: 2026-06-14
knowledge_type: regression-guards
---

# L4 Relay Upstream Connect Timeout — FIXED

## Conclusion

Upstream connection establishment for all three L4 proxies (plain TCP,
TLS-terminated, TLS-passthrough) is bounded by a secure-by-default,
operator-configurable connect timeout.

Fix suggestions of the form "TCP/TLS L4 connect timeout is not independently
configurable / an unreachable backend hangs the downstream connection with no
upper bound / `TransportConnector::new(None)` never passes a connect timeout"
are **not accepted** — the bound now exists.

## Core Rationale

**1. The bound is set on the peer, not the connector**

Pingora's `TransportConnector::new_stream` → `do_connect` honors
`BasicPeer.options.connection_timeout` (wraps the connect future in
`pingora_timeout::timeout`, `pingora-core/src/connectors/l4.rs`). So the fix sets
`peer.options.connection_timeout` at each connect site — there is no need to
change `TransportConnector::new(None)`. This mirrors the HTTP path
(`pg_upstream_peer.rs` `configure_peer_timeouts`).

**2. Secure-by-default with explicit opt-out**

`resolve_tcp_connect_timeout(spec.tcpTimeout)` returns `Some(10s)` even when no
`tcpTimeout` section is configured (`DEFAULT_TCP_CONNECT`, mirrors the idle
sibling's `DEFAULT_TCP_IDLE`). Only an explicit `connectTimeout: "0s"` (any
zero-valued GEP-2257 duration; a bare `"0"` is rejected at admission) opts out
(falls back to the OS TCP retransmit timeout, ~2m). The 10s default matches the
Istio L4 `connectTimeout` baseline.

**3. All three L4 proxy types set the same option**

Plain TCP (`edgion_tcp.rs`), TLS-terminated (`tls/proxy.rs`), and TLS-passthrough
(`tls/passthrough_proxy.rs`) each assign `peer.options.connection_timeout =
resolve_tcp_connect_timeout(...)` right after the existing `tcp_keepalive`
assignment. The TLS doc string of `tcpTimeout` covers all three relay types.

## Fix Suggestions Not Accepted

- "Add a per-route `edgion.io/connect-timeout` annotation to TLSRoute/TCPRoute" —
  per-route granularity is a cross-layer feature (controller annotation parsing +
  per-unit plumbing), not a narrow review fix. The gateway-wide
  `tcpTimeout.connectTimeout` knob covers the operational need (bound an
  unreachable backend). Promote to `tasks/todo/` via brainstorming if per-route
  control is ever genuinely required.
- "Add a Prometheus counter `edgion_l4_upstream_connect_timeout_total{protocol,
  service}`" — the `service` label is unbounded cardinality, which conflicts with
  the project's metrics-explosion-prevention discipline (same class as the
  rejected per-service data-plane counter in `backends/ep-slice-no-port-fail-closed.md`).
  L4 connect failures are already observable in the access log (TCP's
  `UpstreamConnectionFailed` status; the TLS paths record `connect_time`).
- "Change `TransportConnector::new(None)` to `new(Some(timeout))`" — the per-peer
  `connection_timeout` option is the correct, per-route-future-proof site; the
  connector-level timeout is coarser and unnecessary.

## Known related limitations (not defects in this fix)

- **K8s CR pruning**: the hand-maintained `config/crd/edgion-crd/edgion_gateway_config_crd.yaml`
  does not (yet) declare the `tcpTimeout` block, so on the K8s custom-resource path the
  API server prunes `connectTimeout` (and the sibling `idleTimeout`) before the controller
  sees them — the documented config is a no-op there, while file-based config works. This
  is a systemic CRD↔struct drift (also affects `loadBalancing`, `linkSys`, `server.*`),
  tracked in `tasks/todo/crd-struct-sync-gap-edgion-gateway-config.md`, NOT a bug in the
  connect-timeout logic. Do not re-file the connect-timeout finding for this reason.
- **Outlier detection**: bounding the connect means a slow-to-accept backend now books a
  `FailureClass::Gateway` failure at the timeout instead of eventually connecting. This
  only matters when outlier detection is explicitly enabled for the Service (via an
  `EdgionBackendTrafficPolicy`'s `spec.outlierDetection` section; otherwise
  `record_failure_by_addr` is a no-op); attribution and per-attempt recording are
  pre-existing and consistent with the HTTP connect-failure path.

## Re-evaluation Triggers

Re-open this decision only if:

- A new L4 proxy type is added whose connect site does NOT set
  `peer.options.connection_timeout` (it would lack the bound).
- A concrete need for per-route (not gateway-wide) connect timeouts arrives —
  that is a new feature request, not a re-report of this finding.

## Reference Cases

- Schema + helper: `edgion-resources/src/resources/edgion_gateway_config.rs`
  (`TcpTimeout.connect_timeout`, `DEFAULT_TCP_CONNECT`, `resolve_tcp_connect_timeout`)
- Connect sites: `edgion-gateway/src/routes/tcp/edgion_tcp.rs`,
  `edgion-gateway/src/routes/tls/proxy.rs`,
  `edgion-gateway/src/routes/tls/passthrough_proxy.rs`
- Idle-timeout sibling: `skills/04-review/security/regression-guards/l4-relay-idle-timeout.md`
- Docs: `docs/en/ops-guide/configuration/edgion-gateway-config.md` (`spec.tcpTimeout`)
