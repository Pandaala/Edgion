---
name: gateway-api-not-implemented
description: List of Gateway API spec features that Edgion explicitly does not implement, with rationale
---

# Gateway API — Features Not Implemented

This file records Gateway API spec features that Edgion has evaluated and decided not to implement.

## Programmed / Ready status conditions

**Decision: simplified implementation (completed)**

The Gateway API spec lists `Programmed` as a Core required condition, requiring confirmation that the configuration has been programmed into the data plane. Edgion's Controller-Gateway architecture uses asynchronous gRPC configuration sync (the Gateway actively pulls via Watch/List), so the Controller cannot precisely know whether the Gateway has loaded a specific piece of configuration.

A Controller → Gateway HTTP Pull approach was once evaluated (design completed up to Step 03), but it introduced a brand-new communication path, proto changes, delay queues, and so on; complexity was too high and the approach was not adopted. The design document for that approach has been deleted and no archive copy was retained.

The current simplified strategy: after the Controller processes a resource, it sets `Programmed=True` (because the configuration will be synced down to the data plane via gRPC); when there are listener conflicts, unresolved references, invalid TLS configuration, or other anomalies, it sets `Programmed=False`. This satisfies conformance test requirements but does not mean the data plane has actually loaded the configuration.

- The `Programmed` condition is set at both the Gateway level and the Listener level
- See [skills/01-architecture/01-controller/10-status-management.md](../../01-architecture/01-controller/10-status-management.md) for detailed architectural decisions

## Gateway addresses actual-binding reporting / GatewayStaticAddresses / GatewayOptionalAddressValue

**Decision: not implemented**

The `GatewayStaticAddresses` conformance test requires the Gateway to actually bind to the addresses specified in `spec.addresses`. Edgion is a pre-deployed binary; its listening addresses come from TOML configuration and do not dynamically change binding behavior based on Gateway `spec.addresses`. This is also limited by the lack of a data plane → Controller status feedback channel.

Current behavior: `derive_gateway_addresses()` copies `spec.addresses` to `status.addresses`, but the data-plane actual binding addresses do not change (they are still determined by TOML configuration). `SupportGatewayStaticAddresses` and `SupportGatewayAddressEmpty` are not declared.

## Cross-type Hostname conflict detection between GRPCRoute and HTTPRoute

**Decision: not implemented**

The Gateway API spec recommends that hostnames of GRPCRoute and HTTPRoute on the same Listener be unique, with a status condition set on conflict.

In Edgion's current architecture, gRPC requests match GRPCRoute first and fall back to HTTPRoute on no match — a reasonable priority. Coexistence of GRPCRoute + HTTPRoute on the same hostname is a normal use pattern, and major implementations (Envoy Gateway, Istio) do not perform cross-type conflict detection either.

- A related tracking item has not yet been filed under `tasks/` (file a new task to drive this).

## Gateway infrastructure field

**Decision: not yet implemented**

A Gateway API Extended feature that lets users declare desired infrastructure resources (IP addresses, load-balancer types, cloud-provider parameters, etc.) in `GatewaySpec.infrastructure`. The Gateway Controller then automatically creates / configures cloud resources accordingly.

Edgion is a self-deployed gateway and is not responsible for creating cloud infrastructure resources; there is currently no need for cloud-provider integration.

- Will be considered only when there is a need for cloud-provider integration
- A related tracking item has not yet been filed under `tasks/` (file a new task to drive this).

## ListenerSet

**Decision: not yet implemented**

Gateway API v1.5 introduced `ListenerSet`, allowing other resources to append
listeners to an existing Gateway without modifying the `Gateway` directly, with
authorization controlled via `allowedListeners`, namespace constraints, and
`ReferenceGrant`.

Edgion does not plan to implement this capability for the following reasons:

- Edgion leans toward a single-team or single-control-plane usage model where `Gateway` is managed directly; an independent `Gateway` is more natural than a shared `Gateway` + delegated listener appending
- `ListenerSet` significantly increases resource-model complexity: listener merging, priority, conflict detection, route parentRef semantics, attachedRoutes accounting, and status writebacks all need extensions
- The capability mainly addresses governance for multiple teams sharing a single Gateway, which is not a current proxy-capability gap for Edgion

Therefore:

- Do not declare `SupportListenerSet`
- Do not target ListenerSet conformance tests for now
- If clear demand for multi-team sharing of a Gateway emerges in the future, re-evaluate

## HTTPRoute Multiple Mirrors

**Decision: not implemented**

The Gateway API `HTTPRouteRequestMultipleMirrors` allows a single HTTPRoute rule to configure multiple request-mirror targets simultaneously.

Edgion currently retains single-mirror capability but does not plan to extend to multi-mirror, for these reasons:

- Multi-mirror is a relatively weak need; the benefit does not justify the implementation and maintenance cost
- It increases the complexity of request copying, body teeing, async task management, and log/metric aggregation
- For Edgion's current target scenarios, single mirror already covers most shadow / audit-style use cases

Therefore:

- Do not declare `SupportHTTPRouteRequestMultipleMirrors`
- Do not target the `HTTPRouteRequestMultipleMirrors` conformance test for now
- Keep the existing single-mirror capability; do not extend to multi-target mirroring

## TLSRouteModeMixed

**Decision: not implemented**

Edgion supports:

- `TLSRoute` + `TLS listener(mode=Passthrough)`
- `TLSRoute` + `TLS listener(mode=Terminate)`

But explicitly does not support `TLSRouteModeMixed`, that is:

- On the same port and within the same listener capability, dynamically deciding per SNI whether a TLS connection is passthrough or terminate

Reasons:

- Requires a single TCP listener to first read `ClientHello` and then dynamically switch the processing path; architectural complexity rises significantly
- Mixing passthrough / terminate on the same entry point increases the troubleshooting complexity of certificate selection, traffic flow, and security boundaries
- For Edgion's current target scenarios, the standard `Passthrough` and standard `Terminate` already cover the major needs

Therefore:

- `TLSRouteModeTerminate` may be pursued as a follow-up conformance target
- `TLSRouteModeMixed` is not a current implementation target and should not be declared in supported features

## GatewayPort8080 / HTTPRouteParentRefPort / dynamic port binding

**Decision: not implemented**

The `SupportGatewayPort8080` and `SupportHTTPRouteParentRefPort` conformance features both require the gateway to dynamically bind non-standard ports (8080, 8090, etc.).

Edgion's Gateway process binds all ports via `configure_listeners` **at startup**. Port binding is a one-time operation: after Pingora services are initialized, new ports cannot be added at runtime. If a Gateway resource is created after the gateway process has started (such as a temporary Gateway in a conformance test), its listening ports cannot be bound.

- Do not declare `SupportGatewayPort8080` (already removed from `conformance_test.go` and `run.sh`)
- Do not declare `SupportHTTPRouteParentRefPort` (already removed from `conformance_test.go` comments, 2026-03-27)
- In the conformance report, `GatewayWithAttachedRoutesWithPort8080`, `GatewayPort8080`, `HTTPRouteListenerPortMatching`, and `HTTPRouteRedirectPortAndScheme` are marked as skipped
- To support dynamic ports in the future, re-evaluate whether Pingora's `Server::add_service` supports runtime invocation, plus the Controller→Gateway sync timing problem

## BackendTLSPolicy Conformance

**Decision: implemented (declares `SupportBackendTLSPolicy`; does not yet declare `SupportBackendTLSPolicySANValidation`)**

Both data-plane and control-plane status are implemented:
- `ancestorRef` correctly points to the associated Gateway (via the three-step Service → Route → Gateway lookup)
- Supports `caCertificateRefs` of type Secret and ConfigMap
- Conflict detection across multiple Policies on the same Service (sorted by creationTimestamp + name)
- Status reason values match the conformance spec exactly (`InvalidKind`, `InvalidCACertificateRef`, `Conflicted`)
- Route on_change/on_delete automatically requeues the associated BackendTLSPolicy to refresh the ancestor status

Status-reporting limitations: `ancestorRef` pointing to a Gateway means "the control plane considers this Policy to be in effect on this Gateway", but does not mean a data-plane TLS connection will succeed (cert mismatch, backend not supporting TLS, and other runtime issues cannot be reflected in status).

**Scope: HTTPRoute / GRPCRoute only.** BackendTLSPolicy does not apply to TLSRoute. TLSRoute Passthrough transparently forwards the original TLS traffic — the Gateway does not establish a new connection to the backend; TLSRoute Terminate forwards decrypted traffic to the backend by SNI, which is pure stream-level proxying and does not involve L7 backend TLS configuration. For scenarios needing backend TLS (re-encrypt), use HTTPRoute / GRPCRoute + BackendTLSPolicy.

## BackendTrafficPolicy (BTP)

**Decision: not yet implemented (still Experimental; wait for Standard/GA)**

Gateway API v1.3 introduced experimental BackendTrafficPolicy (in the current
v1.5 experimental CRD it is still exposed as `XBackendTrafficPolicy` under
the `gateway.networking.x-k8s.io` API group). Gateway API v1.5 promoted
several features to the Standard channel, but BackendTrafficPolicy is not in
v1.5's Standard/GA promotion list.

The direction of BackendTrafficPolicy is to unify management of backend
traffic behaviors (such as retry budget; later possibly extending to
connection pool, circuit breaker, health check, retry policy).

Edgion has implemented these features in a distributed manner via annotations (`edgion.io/circuit-breaker`, `health_check.rs`, `edgion_gateway_config.rs`); features are complete, but they are not exposed via the standard BTP CRD.

- Will reconsider unifying behind the standard CRD only after BTP graduates from Experimental to Standard/GA
- The current annotation-based approach is usable by users and does not affect actual functionality
- A related tracking item has not yet been filed under `tasks/` (file a new task to drive this).

## ~~frontendValidation standardization~~ ✅ done

The Controller parses `frontend_validation` CA references (Secret/ConfigMap), merges per listener, and sets listener status conditions. The Gateway data plane's `configure_gateway_frontend_mtls()` loads the CA and configures the SSL verify mode: AllowValidOnly uses `PEER | FAIL_IF_NO_PEER_CERT`, and AllowInsecureFallback uses a custom verify callback that always accepts (boringssl FFI). Fully independent from EdgionTls `client_auth`.

## GRPCRoute retry.codes

**Decision: not implemented (architectural limitation)**

The gRPC status code (0–16) lives in the `grpc-status` header in HTTP/2 trailers. Status-code-based retries require reading trailers and making the retry decision before the response stream ends, but the Pingora proxy architecture can read trailers only after the response is complete, at which point retries are no longer possible.

Other GRPCRoute retry fields work normally:

- `retry.attempts`: supported (controls the maximum retry count)
- `retry.backoff`: supported (exponential backoff, capped at 30s)
- `retry.codes`: **not supported**; can be filled in YAML but is ignored at runtime, with a warn-level log emitted

Current behavior: when the Controller parses a GRPCRoute, if `retry.codes` is non-empty it logs a warning `"GRPCRoute retry.codes is not yet supported (requires gRPC trailers interception); codes will be ignored"`, then sets `ParsedRouteRetry.codes` to `None`. Retries trigger only on connection error / upstream unreachable; gRPC status codes are not used to differentiate.

- Workaround: configure a retry policy in the client gRPC SDK (such as `retryPolicy.retryableStatusCodes` in the gRPC service config); the client can read trailers in full and retry by status code
- Related code: `src/types/resources/grpc_route.rs` lines 526–541

## GATEWAY-TLS ConformanceProfile

**Decision: not yet declared**

TLS Passthrough is implemented; TLS Terminate also has the data-plane main path, but TLSRoute conformance tests have not yet been driven through systematically. Status-condition-related logic (ResolvedRefs, Accepted, supportedKinds for TLSRoute) is expected to need fixes, and terminate tests need to be filled in, before the profile can be declared.

- Need to declare `SupportTLSRoute` in SupportedFeatures and pass core passthrough tests before declaring the profile
- `TLSRouteModeTerminate` can continue to be pursued as a follow-up enhancement
- `TLSRouteModeMixed` is explicitly not implemented
- MESH-HTTP / MESH-GRPC: Edgion is not a service mesh; not applicable
- A related tracking item has not yet been filed under `tasks/` (file a new task to drive this).
