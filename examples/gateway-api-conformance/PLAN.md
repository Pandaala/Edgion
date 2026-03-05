# Gateway API Conformance Test — Execution Plan

> Target: Gateway API v1.4.0 Standard Conformance
> Created: 2026-02-14

---

## Current Status (as of 2026-03-01)

### Control-Plane Fixes (all in local branch)

| Issue | Status | Notes |
|-------|--------|-------|
| controllerName mismatch | **Fixed** | `edgion.io/gateway-controller` everywhere |
| GatewayClass Accepted | **Fixed** | `gateway_class.rs` — `update_status` |
| Gateway status.addresses | **Fixed** | Service ClusterIP lookup + `gateway_address` config fallback |
| AttachedRoutes requeue | **Fixed** | `requeue_parent_gateways()` in all route handlers |
| ResolvedRefs validation | **Fixed** | `validate_listener_resolved_refs()` checks secrets + ReferenceGrant |
| SupportedKinds filtering | **Fixed** | `compute_supported_kinds()` respects allowedRoutes.kinds |
| Programmed vs ResolvedRefs | **Fixed** | `Programmed=False` when `ResolvedRefs=False` |
| Listener dynamic creation | **Workaround** | Pre-create Gateway before gateway pod starts |

### Data-Plane Fixes (all in local branch)

| Issue | Status | Notes |
|-------|--------|-------|
| EdgionGatewayConfig missing | **Fixed** | Created `manifests/edgion-gateway-config.yaml` |
| Port conflict (multi-gateway) | **Fixed** | `gateway_base.rs` skips duplicate port bindings |
| Host header port stripping | **Fixed** | `strip_port_from_host()` in request filter |
| Routes not rebuilt on Gateway partial_update | **Fixed** | `rebuild_from_stored_routes()` after Gateway add/update |
| gateway_address hardcoded | **Fixed** | `setup.sh` dynamically patches with Service ClusterIP |

---

## Directory Layout

```
examples/gateway-api-conformance/
├── PLAN.md                             # This file
├── go.mod                              # Go module (gateway-api v1.4.0 dep)
├── go.sum
├── conformance_test.go                 # Go test entry point
├── manifests/
│   ├── gatewayclass.yaml               # GatewayClass 'edgion'
│   ├── edgion-gateway-config.yaml      # EdgionGatewayConfig (referenced by GatewayClass)
│   ├── controller-configmap.yaml       # Controller config for conformance
│   └── conformance-gateway.yaml        # Pre-created Gateways (listener workaround)
└── scripts/
    ├── setup.sh                        # Full setup: CRDs + controller + gateway + manifests
    ├── run.sh                          # Run conformance tests
    └── cleanup.sh                      # Tear down
```

---

## Phase 0 — Infrastructure (this PR)

1. Create directory structure and this plan.
2. Create Go test module with `sigs.k8s.io/gateway-api` v1.4.0 dependency.
3. Write `conformance_test.go` that calls `conformance.RunConformance(t)`.
4. Create Kubernetes manifests:
   - `gatewayclass.yaml` — GatewayClass `edgion` with
     `controllerName: edgion.io/gateway-controller`.
   - `conformance-gateway.yaml` — Pre-created Gateway in
     `gateway-conformance-infra` namespace to work around the listener
     dynamic-creation limitation.
5. Create scripts:
   - `setup.sh` — deploy CRDs, controller (with all-namespace watcher),
     gateway, and conformance manifests.
   - `run.sh` — wrapper around `go test` with sensible defaults.
   - `cleanup.sh` — tear down conformance resources.

## Phase 1 — First Core Conformance Run

1. Build fresh controller/gateway images.
2. Run `scripts/setup.sh` against a Kind/Minikube cluster.
3. Run `scripts/run.sh` with Core features only:
   `Gateway,HTTPRoute,ReferenceGrant`.
4. Collect first-run results to identify failures.

## First Core Run Results (2026-02-28)

> Settings: `--core` (Gateway, HTTPRoute, ReferenceGrant), timeout 30m
> Controller binary: patched `persist_k8s_status` (Patch::Apply → Patch::Merge)

### PASSED (4 tests)

| Test | Notes |
|------|-------|
| GatewayObservedGenerationBump | `observedGeneration` increments correctly |
| GatewayClassObservedGenerationBump | `observedGeneration` increments correctly |
| GatewaySecretReferenceGrantAllInNamespace | ResolvedRefs=True when ReferenceGrant allows |
| GatewaySecretReferenceGrantSpecific | ResolvedRefs=True when specific ReferenceGrant exists |

### FAILED (11 tests, 5 root causes)

| # | Root Cause | Affected Tests | Description |
|---|-----------|---------------|-------------|
| 1 | **Gateway status.addresses missing** | HTTPRouteCrossNamespace, HTTPRouteExactPathMatching, HTTPRouteHeaderMatching, HTTPRouteHostnameIntersection (3 subtests), HTTPRouteHTTPSListener (timed out) | All HTTPRoute data-plane tests wait for `gateway.status.addresses[0]` to send HTTP requests. The controller never sets this field. **Blocks all HTTP traffic tests.** |
| 2 | **AttachedRoutes always 0** | GatewayWithAttachedRoutes (3 subtests) | Dynamically-created Gateways/routes get correct status conditions but `attachedRoutes` stays 0. The `count_attached_routes_for_listener_by_key()` lookup likely runs only during init, not on route create/update events. |
| 3 | **ResolvedRefs not validated** | GatewayInvalidTLSConfiguration (4 subtests), GatewaySecretInvalidReferenceGrant, GatewaySecretMissingReferenceGrant | Controller always sets `ResolvedRefs: True` without checking: cert secret existence, ReferenceGrant permission, resource kind validity. |
| 4 | **SupportedKinds not filtered** | GatewayInvalidRouteKind (2 subtests) | When listener specifies invalid route kinds, controller still returns `[HTTPRoute, GRPCRoute]` instead of empty list / only valid subset. Should set `ResolvedRefs: False` with `InvalidRouteKinds`. |
| 5 | **Dynamic listener modification** | GatewayModifyListeners (2 subtests) | Controller doesn't dynamically add/remove listeners. Known limitation. |

### SKIPPED (not counted — unsupported features)

Mesh (19), BackendTLSPolicy (6), GRPCRoute (5), GatewayHTTPListenerIsolation,
GatewayInfrastructure, GatewayStaticAddresses, GatewayOptionalAddressValue,
GatewayWithAttachedRoutesWithPort8080, HTTPRouteBackendProtocol* (2),
HTTPRouteCORS, HTTPRouteDisallowedKind, plus remaining HTTPRoute tests
not reached before timeout.

---

## Phase 2 — Fix Core Failures (Priority Order)

### P0: Gateway status.addresses (blocks all HTTPRoute tests)

The controller must populate `gateway.status.addresses` so that the conformance
suite knows where to send HTTP requests. Options:
- Use the gateway Pod IP (ClusterIP) as the address.
- Use the gateway Service ClusterIP.
- Use a configured external IP/hostname.

This single fix will unblock **7+ HTTPRoute tests** and many more in extended.

### P1: AttachedRoutes count on dynamic events

The route count works during init (as verified by integration tests) but not
for dynamically-created gateways/routes. The processor needs to re-count
`attachedRoutes` when an HTTPRoute is created/updated/deleted, and persist the
updated Gateway status.

### P2: ResolvedRefs validation for listeners

Add validation in the Gateway handler's `update_status()`:
- Check `certificateRefs` actually exist (Secret lookup).
- Check cross-namespace refs are allowed by ReferenceGrant.
- Set `ResolvedRefs: False` with appropriate reasons when validation fails.

### P3: SupportedKinds filtering by allowedRoutes

When `listener.allowedRoutes.kinds` specifies kinds, validate them against known
route GVKs. If none are valid, set `supportedKinds: []` and
`ResolvedRefs: False / InvalidRouteKinds`.

### P4: Dynamic listener modification (future)

Requires architectural change in the gateway to add/remove listeners at
runtime without restart.

---

## Phase 3 — Expand to Supported Extended Features

After Phase 2 fixes, add supported extended features to the test flags:
`HTTPRouteQueryParamMatching`, `HTTPRouteMethodMatching`,
`HTTPRouteResponseHeaderModification`,
`HTTPRouteBackendRequestHeaderModification`, `HTTPRoutePortRedirect`,
`HTTPRouteSchemeRedirect`, `HTTPRoutePathRedirect`,
`HTTPRouteRequestTimeout`, `HTTPRouteBackendTimeout`, `GRPCRoute`, `TCPRoute`.

## Phase 4 — Implement Missing Features

Refer to `docs/todolist/gateway-api-conformance-todo.md` for the full list.
Priority order:
1. URLRewrite (HostRewrite + PathRewrite)
2. Listener Isolation (421 Misdirected Request)
3. RequestMirror
4. appProtocol (h2c / WebSocket)
5. NamedRouteRule

---

## Configuration Notes

### Controller Config for Conformance

The controller must watch **all namespaces** (conformance creates its own):

```toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"
# watch_namespaces omitted = watch all namespaces
```

### GatewayClass

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: edgion
spec:
  controllerName: edgion.io/gateway-controller
```

### Pre-Created Gateway (Listener Workaround)

The conformance suite creates a Gateway named `gateway-conformance` in the
`gateway-conformance-infra` namespace. Since Edgion only creates listeners at
startup, we pre-create this Gateway and restart the gateway pod after applying
it. The gateway process picks up the listeners on startup.

### Ports

The conformance suite expects the Gateway to listen on port **80** (HTTP) and
**443** (HTTPS). The existing gateway deployment already exposes these ports.
