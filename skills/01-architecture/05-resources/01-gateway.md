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

Kubernetes supplies `GatewayClassOwnership`: the resolved controller name.
The raw referenced GatewayClass must match that name, and its nonempty UID must match the
GatewayClass already published in this deployment's processed cache. Missing, unwatched,
foreign or not-yet-published class incarnations fail closed. Generation and `Accepted`
are not ownership gates. Losing eligibility uses the processor's ordinary deletion path
to withdraw the Gateway, its references, listener registrations and attachment proof.
FileSystem/etcd supply no ownership gate and retain their unscoped Gateway handling.

GatewayClass publication/unpublication requeues matching raw Gateways, including those
previously filtered out of the processed cache. The notification occurs after the class
cache mutation, so ownership checks see the published incarnation or its absence.

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
4. **Resolve inbound PROXY protocol**: strictly parse `edgion.io/inbound-proxy-protocol`, validate
   listener selection, canonicalize an inline policy or resolve its typed `ProxyProtocolTrust`
   EdgionConfigData target, and always write Disabled/Ready/Invalid into the non-CRD transport
   field. Same-namespace ECD refs are allowed; Cluster-visible cross-namespace targets need no
   grant; Namespace-visible cross-namespace targets require the exact Gateway-to-ECD grant.
5. **Replace physical-listener contributions atomically**: publish the complete Gateway
   contribution set to `ProxyProtocolListenerPolicyManager`; requeue only peers affected by a net
   declaration change. Apply fully replaces old contributions and delete replaces with an empty set.

### on_change

1. **Requeue related routes**: via `GatewayRouteIndex`, find all routes whose parentRef points to this Gateway, and when a Listener's hostname or port has changed, requeue them so resolved_hostnames and resolved_listeners are recomputed

### on_delete

- Clear SecretRefManager and CrossNsRefManager registrations
- Clear EdgionConfigData reverse references and atomically remove PROXY policy contributions
- Clear hostname/port caches in GatewayRouteIndex

### update_status

- **Gateway-level Conditions**:
  - `Accepted`: True when there are no validation_errors
  - `ListenersNotValid`: True when there is a port conflict within the same Gateway
- **Listener-level** (each Listener independently):
  - `Accepted`: True when there are no validation_errors
  - `Conflicted`: True when Listener ports conflict within the same Gateway (not detected across Gateways)
  - `ResolvedRefs`: checks the certificateRef kind (must be Secret), whether the Secret exists, whether the Secret data is valid (contains tls.crt/tls.key, non-empty, valid PEM, **cert/key public-key match**), and whether cross-namespace references are permitted by ReferenceGrant
  - Inbound PROXY policy errors merge into the existing condition types without duplicates. Intrinsic
    JSON/selection/policy errors set Accepted false; reference failures set ResolvedRefs false;
    shared physical-listener declaration conflicts set Conflicted true with reason
    `ProxyProtocolPolicyConflict`. Programmed remains false for every invalid case.
  - `supportedKinds`: computes the supported route types based on the protocol (HTTP/HTTPS → HTTPRoute+GRPCRoute, TCP → TCPRoute, UDP → UDPRoute, TLS → TLSRoute); if `allowedRoutes.kinds` contains any unmatched type, reports InvalidRouteKind
  - `attachedRoutes`: obtained from AttachedRouteTracker as the number of attached routes
- **Addresses**: for a dedicated Gateway, report the assigned addresses of its
  UID-matched Service (all LoadBalancer ingress addresses or the ClusterIP).
  A Service still awaiting an address sets `Programmed=False` with
  `AddressNotAssigned`; a requested address not present on the Service sets
  `AddressNotUsable` while still reporting the addresses actually assigned.
  For a shared Gateway, use
  `spec.addresses` when present; otherwise prefer the shared gateway Service,
  then a same-named Service, then the configured default address. Local socket
  binding is independent of these external addresses.

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
