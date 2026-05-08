---
name: resource-backend-tls-policy
description: BackendTLSPolicy resource — upstream mTLS configuration, CA certificate resolution, client certificate, conflict detection.
---

# BackendTLSPolicy resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

BackendTLSPolicy is a Gateway API standard policy resource used to configure upstream TLS connections from the Gateway to backend Services, including CA certificate verification and client certificates (mTLS). It binds to a specific Service via `targetRef`.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs`
- Conflict detection: `src/core/controller/conf_mgr/sync_runtime/resource_processor/backend_tls_policy_ref.rs`
- Gateway ConfHandler: `src/core/gateway/backends/policy/backend_tls/conf_handler_impl.rs`
- Type definitions: `src/types/resources/backend_tls_policy.rs`

## Controller-side processing

### parse

1. **Conflict-detection registration**:
   - Compute old siblings (before clearing, used for requeue)
   - Clear the old target registrations of this BackendTLSPolicy in `BackendTLSPolicyTargetManager`
   - Iterate `spec.target_refs`, generate conflict keys in the format `{ns}/{svc_name}[::sectionName]`, and register
   - Store `creationTimestamp` for conflict-priority ordering
   - Requeue old siblings (when targets change, old siblings need to re-evaluate their conflict status)
2. **Clear stale references**: clear records for this BackendTLSPolicy in SecretRefManager
3. **Resolve CA certificates**: iterate `spec.validation.caCertificateRefs`; for each kind=Secret or kind=ConfigMap reference:
   - Register the reference relationship in SecretRefManager
   - Read data from GLOBAL_SECRET_STORE / ConfigMap Store and populate `spec.resolved_ca_certificates`
4. **Resolve client certificate Secret**: obtain the client certificate reference via `client_certificate_secret_ref()`:
   - Register the reference relationship in SecretRefManager
   - Read Secret data from GLOBAL_SECRET_STORE and populate `spec.resolved_client_certificate`

### on_change

Requeue current siblings (other BackendTLSPolicies sharing the same target) so their conflict status is re-evaluated.

### on_delete

1. Find siblings (before clearing)
2. Clear target registrations and timestamps in `BackendTLSPolicyTargetManager`
3. Clear SecretRefManager references
4. Requeue siblings (after deletion, a previously Conflicted sibling may become Accepted)

### update_status

Use synthetic PolicyAncestorStatus (because BackendTLSPolicy's target is a Service rather than a Gateway):
- `Accepted` (priority: Invalid > NoValidCACertificate > Conflicted > Accepted):
  - validation_errors not empty → `False, Reason=Invalid`
  - All CA refs invalid (kind error + unresolved refs ≥ total refs) → `False, Reason=NoValidCACertificate`
  - Conflict detection: iterate target refs and call `is_policy_winner()`; non-winner → `False, Reason=Conflicted`
  - Otherwise → `True, Reason=Accepted`
- `ResolvedRefs`: validates the kind/group of all caCertificateRefs, whether the CA Secret/ConfigMap is resolved, and whether the client-certificate Secret exists and contains tls.crt and tls.key
- Does not set Programmed/Ready Condition (would require data-plane feedback, not currently supported by the architecture)

**Fixed**: `ancestorRef` now performs a `Service → Route → Gateway` three-step lookup via ServiceRefManager + GatewayRouteIndex and correctly points to the associated Gateway. Route on_change/on_delete automatically requeues the related BackendTLSPolicy to keep ancestor state consistent.

### Conflict detection mechanism

Uses `BackendTLSPolicyTargetManager` (a `BidirectionalRefManager<ResourceRef>` instance), following the same pattern as `ServiceRefManager` and `SecretRefManager`:

- **Conflict key**: `"{ns}/{svc_name}"` or `"{ns}/{svc_name}::{sectionName}"`; different sectionNames do not conflict
- **Priority**: creationTimestamp ASC → `{namespace}/{name}` lexicographic order; the first is the winner
- **Sibling requeue**: parse stage requeues old siblings; on_change stage requeues new siblings; on_delete stage requeues all siblings

## Gateway-side processing

After BackendTLSPolicy is synced to the Gateway side, when initiating a connection to the backend Service:
- Use the CA certificates in `resolved_ca_certificates` to verify the backend's server certificate
- Use the client certificate in `resolved_client_certificate` to provide identity to the backend (mTLS)

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| BackendTLSPolicy → Secret | Secret | caCertificateRefs | CA certificate for verifying the backend's server certificate |
| BackendTLSPolicy → ConfigMap | ConfigMap | caCertificateRefs | CA certificate (ConfigMap form) |
| BackendTLSPolicy → Secret | Secret | clientCertificateRef | Client certificate for mTLS |
| BackendTLSPolicy → Service | Service | targetRef | Target Service the policy binds to |
| BackendTLSPolicy ↔ BackendTLSPolicy | BackendTLSPolicy | BackendTLSPolicyTargetManager | Same-target conflict detection + sibling requeue |
| BackendTLSPolicy ← SecretRefManager | Secret | cascading requeue | Secret changes automatically re-process the policy |
