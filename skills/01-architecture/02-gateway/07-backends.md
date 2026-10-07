---
name: gateway-backends
description: Backend discovery and health checks — joint Service/EndpointSlice generations, runtime identity, the health-check system, BackendTLSPolicy, and endpoint validation.
---

# Backend discovery and health checks

> The backend module is responsible for fetching the actual Pod endpoints behind a Service from Kubernetes,
> and for using the health-check system to automatically eject unhealthy backends and reinstate healthy ones.

## Discovery sources

| Source | Module | K8s resource | Description |
|--------|--------|--------------|-------------|
| EndpointSlice | `discovery/endpoint_slice/` | `EndpointSlice` | Backend endpoint discovery |
| Service | `discovery/services/` | `Service` | Service metadata storage (ClusterIP, port mapping) |

### Discovery data flow

```
Controller (K8s Watch)
  │
  ├── gRPC ConfigSync
  │
  ├── ConfHandler<EndpointSlice> ─┐
  └── ConfHandler<Service>       ─┴─► ServiceDiscoveryCoordinator
                                         └── ServiceSelectorGeneration (ArcSwap)
                                              ├── availability-gated raw projection
                                              ├── canonical port sections
                                              └── eager per-section selector variants
```

`ServiceDiscoveryCoordinator` is the only production mutation owner. Service and EndpointSlice
handlers call one of its four full/partial transaction methods; one transaction serially mutates
both source views, stages the complete source/membership/incarnation demand, reserves the checked
epoch range once, rebuilds all affected Services, and publishes at most one joint generation.
An exhausted reservation still applies deletes and semantic no-ops, but every affected item that
needs a new identity is published unavailable; source identities are never partially committed.
`ServiceStore` and `EpSliceStore` are read-only compatibility facades over that generation, so an
unavailable or unidentified Service has no raw view that a consumer can use to bypass the
fail-closed projection.

Canonical section keys are only Service port number or name. Every valid section contains exact
all/ready endpoint slots plus eagerly compiled RoundRobin, ConsistentHash, LeastConn, and EWMA
selectors. Equivalent same-socket origins merge into a deterministic sorted origin set. The
native Route compiler takes an actual Route's effective field loan and stores
one `CompiledServiceTarget` containing the canonical policy target, Service key and
checked `ServiceSectionKey::Number`. Missing or invalid declared ports produce an
Invalid slot before discovery. HTTP/GRPC reject it before backend plugins; HTTP
returns `BackendUnsupportedKind`/500 for that slot. A valid HTTP/GRPC declared
section that becomes unavailable at runtime still returns
`BackendPortResolutionFailed`/503. L4 cold compilation uses the same owning target and checked section. Its invalid
slots fail terminally as `UnsupportedBackend` (denial takes precedence); valid
live endpoint/section failures retain TCP/UDP failure and TLS retry behavior.
L4 selectors borrow original slots and retain live degradation without capturing
BackendTLSPolicy or backend traffic policy. No selector may pick
the Service's first port, a leftover slice port or another section. The selected slot already carries `serving_port` on `backend.addr`; there is
no request-path port rewriter. `appProtocol` is published on `ServicePortIdentity` and read from
the proven section, not a live ServiceStore scan. Health probes use
`AvailableServiceProjection::canonical_probe_backends()` (union of Resolved `all_slots`,
deduped by runtime identity). Session persistence is keyed per section.

DirectEndpoint borrows `CompiledBackend::permitted_service()`, the same private
compiled Service target used by ordinary HTTP selection and final peer resolution. Denied refs and invalid/missing
Service ports have no permission. One captured discovery generation supplies the section's
precompiled numeric IP/port index. An explicit endpoint port must match that section; an omitted
port requires one serving port for the IP. This lookup does not read raw Kubernetes resources,
scan endpoints, format keys, parse addresses, or take a blocking lock. Direct overrides include
`all_slots` (including unready endpoints) by explicit product choice for targeted debugging;
normal selectors retain their existing eligibility rules. The proven section name selects the
captured BackendTLSPolicy tri-state. The resulting address and policy survive retries, and the
final peer builder enforces TLS and endpoint address safety. Endpoint-derived addresses must be
inet sockets; generic explicitly configured Unix upstreams retain their separate contract.

## Health-check system

### Component layout

```
health/
├── check/
│   ├── manager.rs        # HealthCheckManager — manages a check task per service
│   ├── probes.rs         # Plaintext probe implementations (HTTP/TCP/gRPC)
│   ├── config_store.rs   # HealthCheckConfigStore — health-check configuration store
│   └── status_store.rs   # HealthStatusStore — health status store
└── mod.rs
```

### HealthCheckManager

- Global singleton (`HEALTH_CHECK_MANAGER`, LazyLock).
- Maintains one background tokio task (`JoinHandle<()>`) per service.
- `reconcile_service(service_key)` — creates/updates/deletes the check task based on configuration.
- **Configuration source**: `HealthCheckConfigStore` is a single map keyed by
  Service key (`namespace/service`) — there are no source tiers anymore. The
  `EdgionBackendTrafficPolicy` policy store
  (`backends/policy/backend_traffic/store.rs`) is the **sole writer**: on every
  policy full_set/partial_update it resolves the oldest-wins winner per
  targeted Service key and calls `set_service_config` +
  `hc_manager.reconcile_service(key)`. There is no annotation-parsing path —
  `health/check/annotation.rs` and the EndpointSlice-tier override were
  removed; see [Backend Traffic Policy](#backend-traffic-policy-edgionbackendtrafficpolicy) below.
- Without configuration, deletes the task and unregisters health status.

### Probe types

| Type | Function | Description |
|------|----------|-------------|
| HTTP | `probe_http()` / `probe_http_with_client()` | Plaintext HTTP GET check, supports a custom client |
| HTTPS | `EncryptedProbeTransport::https()` | Verified HTTP GET over health-local TLS, pinned to the exact endpoint |
| TCP | `probe_tcp()` | Plain TCP connection check; it does not perform a TLS handshake or application-level validation |
| gRPC | `probe_grpc()` | Plaintext gRPC Health Check Protocol (`grpc.health.v1.Health/Check`), implemented with hand-written prost structs, no proto file needed |
| GRPCS | `EncryptedProbeTransport::grpcs()` | Health RPC over verified health-local TLS, h2 ALPN and HTTPS origin |

### Transport ownership

Active health-check transport is owned entirely by
`EdgionBackendTrafficPolicy.spec.healthCheck.active`. It is independent of the
Service's request-path `BackendTLSPolicy`: the health manager does not use that
policy to select a protocol, inherit CA or client identity, set SNI, or trigger
task replacement. This is deliberate because a probe may use a different
protocol and port from application traffic; a TLS application backend commonly
uses a plaintext HTTP endpoint or a TCP connect probe.

The type names are exact transport contracts: `http` and `grpc` are plaintext,
`tcp` stops after connect, and `https`/`grpcs` require their own verified TLS
configuration. The health-local `OutboundTlsLocal` resolves through
`resolve_outbound_tls_with_defaults` with explicit default roots and no global
identity. `build_verified_outbound_tls` enforces CA, expiry, hostname, additional
SAN constraints and optional client authentication. This uses the health client's
supported rustls runtime and does not touch the Pingora upstream-rustls seam.

HTTPS clients use the canonical outbound factory and endpoint-pinned resolver;
GRPCS uses a pinned tokio-rustls connector with h2 ALPN and the tonic health codec.
Each exchange has its own client/channel, while the task shares its immutable
TLS configuration. No connection pool can cross endpoint or TLS generations.
Proxy pickup is disabled, redirects are denied, and errors expose stable classes.

Controller parse clears resolved material and old Secret dependencies before
resolving the current health-local refs. Missing references stay registered for
requeue. TLS failures are separate from attachment eligibility: the valid target
marker and winning check remain, while status reports the error and the encrypted
transport fails. This prevents missing trust from silently removing checking.

A local publication gate serializes health-config replacement with encrypted
probe polling. Every poll verifies the captured identity under the gate; no guard
survives Pending. Old queued probes cannot start after publication. Already
submitted I/O is in flight and cancelled best effort; coordinator identity checks
reject stale results. Complete active-config equality includes resolved material.

A standalone TLS-handshake type, custom checkers and separate healthy/unhealthy
intervals remain out of scope. See the [transport decision](../../04-review/backends/intentional-design/active-health-check-transport-is-independent.md).

### Runtime health identity

Every live canonical slot has an immutable `EndpointRuntimeIdentity` containing its Service,
section proof, socket, membership incarnation, and endpoint proof. Same-socket different-section
identities stay isolated. A same-socket proof change creates a fresh
runtime state and may reuse only the connection-count, EWMA, and circuit-breaker-inflight counters.
Health and outlier eligibility are new identity-bound state. Probe and request completions carry
the expected membership and endpoint identities plus the exact health/outlier policy identity;
the Coordinator rejects late writes after an endpoint or policy update. Activation and completion
share the endpoint-local completion lock, so retirement waits for an already-linearized write and
then freezes it. Detached old state keeps its last eligibility, never auto-recovers, and is never
widened for an in-flight request. A membership change always creates a new wrapper and runtime
state; only its three transport counters may be shared with the same socket.

## Circuit Breaker

Passive ejection mechanism to prevent cascading failures. References a slimmed-down version of Envoy's `outlier_detection`.

### Component layout

```
circuit_breaker/
├── config.rs     # re-exports CircuitBreakerConfig / OutlierDetectionConfig
│                 # (the struct + normalize()/validate() now live in
│                 # edgion-resources::resources::outlier_detection)
├── store.rs      # CircuitBreakerStore — DashMap-based ejection state store
└── mod.rs
```

### Core mechanics

- **Consecutive failure counting**: connect failures / proxy errors / upstream 5xx each count as one; non-5xx responses reset the counter.
- **Auto ejection**: once consecutive failures hit the threshold, mark `ejected_at`; LB selection skips the backend.
- **Auto recovery**: when `is_ejected()` is checked and the cooldown has expired, the ejection state is cleared automatically.
- **Maximum ejection ratio**: prevents all backends from being ejected at once (0 = no limit).

### Data store

`CircuitBreakerStore` (global LazyLock singleton):
- Outer `DashMap<String, Arc<ServiceCbState>>`: service_key → configuration + endpoint state.
- Inner `DashMap<u64, EndpointState>`: backend_hash → consecutive-failure count + ejection time.
- `record_success` deletes the entry directly (not reset to 0) to prevent memory growth.
- Services without CB configuration have no entry; `has_service()` returns false and skips immediately.

### Failure-counting hooks

| Hook | File | Trigger |
|------|------|---------|
| `fail_to_connect` | `pg_fail_to_connect.rs` | Connection failed |
| `error_while_proxy` | `pg_error_while_proxy.rs` | Proxy error (non retry-on-status) |
| `upstream_response_filter` | `pg_upstream_response_filter.rs` | 5xx records failure, non-5xx records success |

### LB integration

`is_backend_healthy()` adds a CB ejection check after the health check; it applies automatically to all LB algorithms.

### Lifecycle

- `CircuitBreakerStore.set_service_config` is written **only** by the
  `EdgionBackendTrafficPolicy` policy store (oldest-wins winner's
  `spec.outlierDetection`, normalized) — see
  [Backend Traffic Policy](#backend-traffic-policy-edgionbackendtrafficpolicy)
  below. There is no annotation parsing anymore.
- Coordinator activation revokes removed or proof-replaced endpoint identities before exposing the
  new complete membership. Identity-keyed cleanup prevents a late completion for the old socket
  occupant from mutating the replacement.
- A Service/EndpointSlice `full_set` resync (e.g. every gateway reconnect)
  must **not** delete the per-service CB/HC/panic/LB policy config — that
  would wipe policy-written entries with nothing to re-apply them. Only the
  policy `ConfHandler` creates/updates/deletes those entries, independent of
  whether the Service currently exists.

### Prometheus metrics

Service degrade selections use `edgion_lb_degrade_selections_total{service}`.
Runtime snapshots retain the cached Counter; the Gateway scrape boundary pins
live handles against exporter idle eviction. Last-owner release removes the
weak registry entry without blocking; on contention, the next registration,
scrape, or stats call cleans deferred dead entries. Store stats expose
`service_discovery.degrade_metric_handles` and bounded Service key samples,
including owners retained by in-flight old snapshots. See
[the lifecycle regression guard](../../04-review/lb/regression-guards/degrade-selection-counter-ownership.md).

| Metric | Type | Label | Recorded when |
|--------|------|-------|---------------|
| `edgion_circuit_breaker_ejections_total` | Counter | `service` | An ejection succeeds |
| `edgion_circuit_breaker_ejected_endpoints` | Gauge | `service` | +1 on ejection, -1 on recovery / cleanup |
| `edgion_circuit_breaker_rejections_total` | Counter | `service` | LB selection hits an ejected backend |

## Backend Traffic Policy (EdgionBackendTrafficPolicy)

Full external schema: [02-features/03-resources/20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md).

```
policy/
├── backend_traffic/
│   ├── store.rs             # BackendTrafficPolicyStore — one ArcSwap generation
│   ├── conf_handler_impl.rs # ConfHandler<EdgionBackendTrafficPolicy>
│   └── mod.rs                # current_lb_policy_for_admin() — admin/snapshot helper
└── mod.rs
```

`BackendTrafficPolicyStore` (`get_backend_traffic_policy_store()`, global
singleton) is the single owner of typed Service and EdgionBackend policy
config. One `ArcSwap` generation publishes `{generation, policies, reverse_index,
effective}` together. Equivalent effective-policy `Arc`s (retry budget,
logical CB, AI outlier handle) are retained across no-op resyncs. AI planning
captures one effective Arc before candidate iteration. HTTP/gRPC Service
requests capture the same Arc at first `begin_upstream_attempt` onto
`BackendContext.effective_policy`, only after obtaining the permitted selected
HTTP/GRPC target (or DirectEndpoint's pinned route slot) and verifying the
context projection. A context key alone performs no policy lookup. They do **not** re-read the store on later
attempts. A missing capture means “no policy”, not “read the live generation”.
The store is also the **sole writer** of `HealthCheckConfigStore`, Service
outlier configuration, and the `DegradeThresholdStore` per-service override.
The shared section selector receives `ServiceDegradePolicy::CapturedOverride`
from the HTTP/gRPC request's effective policy. An absent override uses the current
global default; an explicit zero disables degrade. It never substitutes the live
Service override. Health/outlier state and membership remain live per selection.
The policy store replaced the former
route-fed, ref-counted `lb/lb_policy/PolicyStore` (deleted) and the later
request-visible DashMaps.

- **Arbitration** (`reapply_target(PolicyTargetKey)`): among eligible cached
  policies whose `targetRefs` include the typed target, pick the minimum
  `precedence_key` (oldest `creationTimestamp`, then name) — same rule as
  `BackendTLSPolicy`. The winner's request-visible sections are published in
  one `EffectiveBackendTrafficPolicy`. Health/outlier **status** stays live
  and is not frozen into the generation.
- **`full_set_policies` / `partial_update_policies`**: `partial_update` is a
  real incremental diff (ConfHandler iron rule) — it patches only the reverse
  index lists touched by the changed policies' old ∪ new `targetRefs`, then
  publishes all affected targets together. Side effects run after
  `ArcSwap::store` and read the passed `next` snapshot.
- **EPS rebuild on LB change**: when a Service winner's LB algorithm changes,
  publication triggers
  `config_client.trigger_endpoint_slice_update_event(service_key)` so
  the affected Service generation rebuilds against the new algorithm.
- Admin `/store/stats` exposes the resolved LB map as `lb_policies` (a
  `service_key → algorithm` map) from the current snapshot — this is the field
  that replaced the old `policy_store` stat.

Accepted residuals (not mixed-generation Service HTTP/gRPC reads):

- TCP/UDP/TLS passthrough and `request_mirror` use the shared section selector
  with `ServiceDegradePolicy::Live`, reading the active runtime snapshot's
  already resolved threshold without the HTTP/gRPC captured-policy context. The former
  `select_roundrobin_backend_key`/`degrade_engaged_l4` helpers have been removed.
- `request_mirror` uses the backend address as Host and does **not** apply
  `upstreamAuthority`.
- The EdgionGatewayConfig panic default is a **cross-store** window versus the
  captured EBTP override. Do not merge EGC into the EBTP generation.

## BackendTLSPolicy

Manages upstream TLS policy configuration, stored in `BackendTLSPolicyStore`.
BoringSSL supports custom CA trust and client-certificate authentication; rustls
currently supports only the system CA store and standard hostname verification:

```
policy/
├── backend_tls/
│   ├── backend_tls_policy_store.rs  # BackendTLSPolicyStore (storage + queries)
│   ├── conf_handler_impl.rs         # ConfHandler implementation
│   └── mod.rs
└── mod.rs
```

- Get the global instance via `get_global_backend_tls_policy_store()` and
  capture its immutable, generation-bearing `BackendTLSPolicySnapshot`.
- Full publication rebuilds the complete target view. Partial publication uses
  structurally shared persistent maps for policies and typed target buckets,
  applies `remove > update > add` for defensive overlapping batches, and
  recomputes only the union of targets named by each changed policy's old and
  new generations. Each target bucket retains all valid and invalid candidate
  keys so removing or moving a winner can promote the next candidate without a
  global policy scan. The raw map and derived target buckets remain one atomic
  `ArcSwap` generation; complete resource equality suppresses no-op generations.
- Lookup returns a tri-state: `Absent`, `Effective`, or `Invalid`. Occupancy
  of the most specific attachment bucket wins: a valid exact-section winner is
  used; any exact-section candidate (including an invalid one) blocks
  whole-target fallback; otherwise the whole-target bucket is consulted the
  same way. `get_policy` is gone so callers cannot collapse invalid into
  plaintext.
- Service section input comes from the published discovery generation
  (`ServiceSectionKey::Number` → `identity.name`), not `resolve_port_name`.
  A number-only resolved port (`identity.name = None`) uses whole-target only
  and does not apply the `any_section_scoped ⇒ Invalid` penalty. An unproven
  mapping means a valid declared numeric port whose Service is unavailable or
  whose live section cannot be proven. It is `Invalid` when any section-scoped
  candidate names that target. Missing or invalid declared ports receive no
  native Service target or TLS lookup capability during cold compilation,
  including FileSystem/etcd input. Guessing a section from the Service spec
  would validate a different port than the one actually dialed.
- EdgionBackend AI targets ignore port and look up whole-target only.
- HTTP, gRPC, and AI carry `BackendTlsPolicyLookup` on the selected backend.
  `Invalid` fails closed (`EdgionErrorCode::BackendTlsPolicyInvalid`) before
  peer construction. `Absent` keeps the previous no-policy behavior (HTTP/gRPC
  plaintext; AI may still use endpoint `use_tls` / SNI). `Effective` builds
  the TLS peer and always sets Pingora `group_key` from the profile
  fingerprint (first 8 bytes of SHA-256), including when the SAN list is empty.
- The Controller publishes `resolvedTargetClass` only after target and TLS
  references are valid. Marker-absent, class-mismatch, multi-`targetRef`,
  identity errors, and incomplete custom CA are invalid-applicable, not omitted.
- Configure the HttpPeer's TLS connection parameters. Custom CA certificates and
  client certificate/key identity are applied only by the BoringSSL build; they
  are outside the current rustls support boundary.
- BackendTLS hostname controls certificate verification and SNI; it does not
  rewrite the HTTP Host authority.
- Secret key constants: `CA_CERT`, `CERT`, `KEY`.

## Publication and sticky state

There is no load-balancer preload, selector hydration, or lazy selector cache. Reconciliation
eagerly compiles all selector variants before publishing the generation. Each available projection
also owns a stable per-Service runtime cell whose active membership is one lock-free ArcSwap
snapshot. Sticky-affinity hits are lock-free; a miss builder takes only the cell's miss lock and
rechecks both the published generation and membership before installing its snapshot.

## Endpoint validation

`validate_endpoint_in_route()` provides a security check:

- Verifies that the target IP is a legitimate endpoint within the route's BackendRefs.
- Prevents SSRF: ensures requests can only be routed to endpoints the route has authorized.
- Parameters: target IP, optional endpoint port, compiled route backend slots,
  the captured discovery generation, TLS policy snapshot, and `inherit_tls`.
  Target authority comes from each slot's permitted compiled Service target;
  there is no detached raw-reference or route-namespace input.
- Returns: `ValidatedDirectEndpoint` with the original backend slot, proven address,
  captured TLS policy and appProtocol, or `Err(reason)`.
- Used by `DirectEndpoint`, which selects a backend by endpoint address. Name-addressed jumps
  (`DynamicInternalUpstream`, `Canary`) do NOT use this function — they resolve through
  `RouteRules::find_backend_by_name`, the single name matcher; see
  [`skills/04-review/routing/regression-guards/denied-named-jump-fails-closed.md`](../../04-review/routing/regression-guards/denied-named-jump-fails-closed.md).

## Directory layout

```
edgion-gateway/src/backends/
├── mod.rs                    # module exports
├── discovery/                # backend discovery
│   ├── endpoint_slice/       # EndpointSlice discovery
│   ├── services/             # Service compatibility facade
│   └── coordinator.rs        # Joint source, selector, proof, and runtime generation
├── health/                   # health checks
│   └── check/                # check system
├── circuit_breaker/          # circuit breaker (passive ejection)
├── degrade_threshold.rs        # degrade-threshold store + resolution
├── policy/                   # backend policies
│   ├── backend_tls/          # BackendTLSPolicy (upstream mTLS)
│   └── backend_traffic/      # EdgionBackendTrafficPolicy (HC/CB/panic/LB arbitration)
└── validation.rs             # endpoint validation (SSRF prevention)
```
