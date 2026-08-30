---
name: resource-gateway
description: Gateway resource architecture — Listener definitions, port management, TLS certificate binding, Route attachment, Status management.
---

# Gateway resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

Gateway is Edgion's core resource. It defines the gateway's Listeners (listening ports, protocols, hostnames, TLS configuration) and serves as the attachment point for all route resources.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs`
- Gateway ConfHandler: `edgion-gateway/src/runtime/handler.rs` (drives `GatewayStore`, `ListenerGatewayInfoStore`, and the per-port route managers)
- Type definitions: `edgion-resources/src/resources/gateway.rs`

## Controller-side processing

### filter

Filters by `gatewayClassName`. If `gateway_class_name` is configured as `Some(name)` (K8s mode), only matching Gateways are processed; if `None` (FileSystem mode), all Gateways are processed.

### parse

1. **Clear stale references**: clear records for this Gateway from SecretRefManager and CrossNsRefManager (handles the update case)
2. **Resolve the backend client certificate**: resolve
   `spec.tls.backend.clientCertificateRef`, register it in SecretRefManager, and populate
   `resolvedClientCertificate` for conf_sync. Cross-namespace references require a
   ReferenceGrant.
3. **Resolve listener TLS certificates**: iterate over each Listener's `tls.certificateRefs`:
   - Register the reference relationship in SecretRefManager (Secret changes will automatically requeue this Gateway)
   - If it is a cross-namespace reference, also register it in CrossNsRefManager
   - Read Secret data from `ctx.runtime().secret_store` and populate the resolved TLS transport field
   - If the Secret has not yet arrived, do not error out; SecretRefManager guarantees automatic re-processing later

### on_change

1. **Requeue related routes**: via `GatewayRouteIndex`, find all routes whose parentRef points to this Gateway, and when a Listener's hostname or port has changed, requeue them so resolved_hostnames and resolved_listeners are recomputed

### on_delete

- Clear SecretRefManager and CrossNsRefManager registrations
- Clear hostname/port caches in GatewayRouteIndex

### update_status

- **Gateway-level Conditions**:
  - `Accepted`: True when there are no validation_errors
  - `ListenersNotValid`: True when there is a port conflict within the same Gateway
- **Listener-level** (each Listener independently):
  - `Accepted`: True when there are no validation_errors
  - `Conflicted`: True when Listener ports conflict within the same Gateway (not detected across Gateways)
  - `ResolvedRefs`: checks the certificateRef kind (must be Secret), whether the Secret exists, whether the Secret data is valid (contains tls.crt/tls.key, non-empty, valid PEM, **cert/key public-key match**), and whether cross-namespace references are permitted by ReferenceGrant
  - `supportedKinds`: computes the supported route types based on the protocol (HTTP/HTTPS → HTTPRoute+GRPCRoute, TCP → TCPRoute, UDP → UDPRoute, TLS → TLSRoute); if `allowedRoutes.kinds` contains any unmatched type, reports InvalidRouteKind
  - `attachedRoutes`: obtained from AttachedRouteTracker as the number of attached routes
- **Addresses**: derived in priority order: spec.addresses > ClusterIP of a Service with the same name > the default address configuration > "0.0.0.0"

## Gateway-side processing

After the Gateway resource is synced to the Gateway side, it is used to configure Pingora's listeners. Each Listener is identified by a `ListenerKey` (formatted `"{addr}/{protocol}"`); `ListenerGatewayInfoStore` manages aggregated Gateway information per `ListenerKey` (including each Listener's hostname constraint and allowedRoutes via the per-port `GatewayInfoIndex`). TLS certificates are stored in the certificate-matching engine, and a request selects a certificate by matching ListenerKey + SNI.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| Gateway ← Routes | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute/UDPRoute | parentRef | Routes attach to a Gateway's Listener via parentRef |
| Gateway → Secret | Secret | `spec.tls.backend.clientCertificateRef` | Default client identity for upstream mTLS |
| Gateway → Secret | Secret | certificateRefs | Listener TLS configuration references certificates in Secrets |
| Gateway ← EdgionTls | EdgionTls | parentRef | EdgionTls binds to a Gateway via parentRef |
| Gateway ← GatewayClass | GatewayClass | gatewayClassName | Gateway references a GatewayClass |
| Gateway ← EdgionGatewayConfig | EdgionGatewayConfig | global | Global configuration affects all Gateways |
