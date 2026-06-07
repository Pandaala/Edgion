---
module: routing
title: Request mirroring uses a separate reqwest client that bypasses LB policy and BackendTLSPolicy
severity: medium
envoy_reference: Envoy request_mirror_policy — mirror traffic is sent via the same upstream cluster machinery (LB, health-check, CB, TLS); the cluster is a first-class reference, not a raw HTTP client
---

# Request mirroring uses a separate reqwest client that bypasses LB policy and BackendTLSPolicy

## Envoy's design (the proven baseline)

Envoy's `request_mirror_policy` sends a shadow copy of the request to a named cluster.
Because the mirror target is a cluster reference, the shadow request passes through the
full cluster machinery: load balancing (round-robin, EWMA, etc.), health-check-aware
endpoint selection, outlier detection / circuit breaking, and cluster-level TLS
configuration. Envoy does not instantiate a separate HTTP client for mirrors; the
shadow request is a first-class upstream request, just detached from the primary
response path.

## Edgion's current design

`RequestMirrorPlugin::new` (`request_mirror/plugin.rs`, lines 31–68) constructs a
`reqwest::Client` at plugin instantiation time. At runtime (`run_request`, lines
180–270), it calls `select_roundrobin_backend_key` (a plain round-robin over discovered
endpoints) and then sends the mirror request via `reqwest`. Key properties:

- `select_roundrobin_backend_key` — bypasses the active LB policy (EWMA, LeastConn,
  ConsistentHash) configured on the service; always round-robin.
- Health-check status and circuit-breaker ejection ARE consulted: `select_roundrobin_backend_key`
  calls `is_backend_healthy`, which checks both the active health-check store and the
  circuit-breaker store. Unhealthy or CB-ejected endpoints are skipped.
- `BackendTLSPolicy` is not applied; the scheme is determined by a port heuristic
  (`port == 443` → HTTPS) rather than by policy lookup (plugin.rs line 130).
- Per-plugin `reqwest::Client` is created at construction — each `EdgionPlugins` reload
  creates a new client, discarding the connection pool.

## The problem / gap

Mirror traffic sent through a raw reqwest client is invisible to the rest of the
gateway's operational infrastructure:

1. **LB policy divergence**: for services using EWMA or LeastConn, the mirror picks
   endpoints without respect to those policies, creating uneven load on the backend.
2. **TLS misconfiguration**: if the mirror target service requires mTLS via
   `BackendTLSPolicy`, the mirror will silently use plain HTTP (when port != 443) or
   plain HTTPS without the client certificate, causing connection failures or mTLS
   authentication errors.
3. **Pool fragmentation**: a new `reqwest::Client` per plugin-reload means the mirror
   connection pool is destroyed on every hot-reload, causing a connection storm to the
   mirror backend on each config update.

## Impact

- Operators who configure `BackendTLSPolicy` (mTLS client cert) on the mirror target
  service are silently getting connections without the client cert; the backend rejects
  them. This is a silent, hard-to-diagnose failure.
- For services using EWMA or LeastConn, the mirror always uses round-robin, diverging
  from the load distribution operators configured.
- In production deployments with frequent `EdgionPlugins` hot-reloads, connection pool
  churn to mirror backends is O(reload frequency).
- The `max_concurrent` semaphore bounds throughput but does not protect against
  connection storms from pool teardown on reload.

## Verification note

Verified against actual source (`request_mirror/plugin.rs`). `RequestMirrorPlugin` stores a
`reqwest::Client` as a struct field (line 28), constructed in `new()` (lines 51–56). The
client is not shared across plugin instances — every `PluginRuntime` rebuild creates a new
`RequestMirrorPlugin` via `plugin_runtime.rs` line 716, discarding the previous client and
its connection pool.

`resolve_target` (line 117) calls `select_roundrobin_backend_key` (line 137), which is a
plain round-robin over service endpoints, bypassing EWMA/LeastConn/ConsistentHash LB
policies configured on the backend service.

**Correction to original finding**: `select_roundrobin_backend_key` (`backends/selector.rs`
line 73) DOES call `is_backend_healthy`, which checks both the active health-check store
AND the circuit-breaker ejection store (`selector.rs` lines 42–54). Unhealthy or
CB-ejected endpoints are filtered out. The claim that health-check failures and circuit
breakers are ignored is incorrect; those protections ARE active for the mirror path.

The scheme heuristic is at `plugin.rs` line 130:
`let scheme = if port == 443 { "https" } else { "http" };` with a `// TODO(Phase 2)` comment
explicitly acknowledging the BackendTLSPolicy gap. The BackendTLSPolicy bypass is the
primary concrete defect: a service requiring mTLS will never receive a client certificate
from the mirror, causing authentication failures on the backend side.

## Suggested direction

Route mirror requests through the same `EdgionBackend` / `get_peer` machinery used
for primary requests. Concretely:

- Resolve the mirror backend via `BackendTLSPolicy`-aware service lookup (same as
  `select_http_backend`).
- Issue the mirror request as a real Pingora upstream connection (or at minimum reuse
  the Pingora HTTP client stack with the circuit-breaker and LB path).
- If a standalone reqwest client is kept for implementation simplicity, ensure the
  `BackendTLSPolicy` query is wired in at plugin construction time, and reuse the client
  across reloads by storing it in a stable `OnceLock` or global keyed by service.
