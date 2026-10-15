---
name: resource-backend-tls-policy
description: BackendTLSPolicy resource — upstream mTLS configuration, CA certificate resolution, client certificate, conflict detection.
---

# BackendTLSPolicy resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

BackendTLSPolicy is a Gateway API standard policy resource used to configure upstream TLS connections from the Gateway to direct backend targets, including CA certificate verification and client certificates (mTLS). Edgion accepts core `Service` and AI `edgion.io/EdgionBackend` targets.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs`
- Conflict detection: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/backend_tls_policy_ref.rs`
- Gateway ConfHandler: `edgion-gateway/src/backends/policy/backend_tls/conf_handler_impl.rs`
- Type definitions: `edgion-resources/src/resources/backend_tls_policy.rs`

## Controller-side processing

### parse

1. **Typed target evaluation and conflict registration**:
   - Compute old siblings (before clearing, used for requeue)
   - Clear the old target registrations of this BackendTLSPolicy in `BackendTLSPolicyTargetManager`
   - Require exactly one targetRef and derive a typed `(kind, namespace, name, section)` identity
   - Register the canonical dependency before existence lookup, so target creation can self-heal a missing policy
   - Resolve Service existence or the validated raw EdgionBackend capability; all EdgionBackend variants reject `sectionName`, and external/AI targets reject plaintext endpoints
   - Set the schema-hidden `resolvedTargetClass` marker and enter conflict arbitration only after complete eligibility
   - Recompute schema-hidden `preparedPrecedence` from complete source `creationTimestamp`, and store that same instant for conflict-priority ordering
   - Requeue old and new siblings only when the eligible target set or complete creation instant changes
2. **Clear stale references**: clear records for this BackendTLSPolicy in SecretRefManager
3. **Resolve CA certificates**: use `BackendTlsRefs` on the actual policy after the shared native eight-reference bound:
   - Borrow each CA ordinal and validate its native local target before effects
   - Use `fetch_bound_secret` / `fetch_bound_configmap` to register ordinary typed dependencies before authorization and cached acquisition
   - Preserve actual source metadata and ConfigMap provenance, then populate `spec.resolved_ca_certificates` after field loans end
4. **Resolve client certificate Secret**: use the same actual `BackendTlsRefs` owner and its client option field:
   - Preserve the existing option parser's trim and same-namespace DNS-name grammar
   - Use the bound Secret pipeline, then populate `spec.resolved_client_certificate` after the owner loan ends
   - Status and cold Gateway publication share typed reference/material pairing; missing or mismatched material cannot satisfy another field

### on_change

Sibling requeue occurs during parse, gated on eligible target or timestamp changes. The on_change hook notifies dependent EBTPs; it does not unconditionally requeue TLS siblings.

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

- **Conflict key**: typed target kind + namespace + name + optional section; same-name Service and EdgionBackend targets never collide
- **Priority**: creationTimestamp ASC → `{namespace}/{name}` lexicographic order; the first is the winner
- **Sibling requeue**: parse stage requeues old/new siblings on precedence-relevant changes; on_delete requeues remaining siblings
- **Section lookup**: an exact Service-port winner is preferred, then the whole-Service winner; another section is never a fallback. EdgionBackend is whole-target only.

## Gateway-side processing

The Gateway keeps one immutable typed policy snapshot with a monotonic generation. It indexes only policies whose Controller marker matches their typed target. Equivalent updates retain the policy Arc and generation; material changes publish once and old captured Arcs remain valid for in-flight requests.

When initiating a connection to a Service or AI backend:
- Use the CA certificates in `resolved_ca_certificates` to verify the backend's server certificate
- Use the client certificate in `resolved_client_certificate` to provide identity to the backend (mTLS)
- Use the same canonical peer builder for both target classes

Service freezes the TLS winner when the logical Service is selected. AI captures the winner in its request-stable candidate plan. AI keeps the provider endpoint Host authority; the policy hostname owns SNI and certificate authentication only.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| BackendTLSPolicy → Secret | Secret | caCertificateRefs | CA certificate for verifying the backend's server certificate |
| BackendTLSPolicy → ConfigMap | ConfigMap | caCertificateRefs | CA certificate (ConfigMap form) |
| BackendTLSPolicy → Secret | Secret | clientCertificateRef | Client certificate for mTLS |
| BackendTLSPolicy → Service | Service | targetRef | Target Service the policy binds to |
| BackendTLSPolicy → EdgionBackend | EdgionBackend | targetRef | Direct AI target; no nested-Service policy inheritance |
| BackendTLSPolicy ↔ BackendTLSPolicy | BackendTLSPolicy | BackendTLSPolicyTargetManager | Same-target conflict detection + sibling requeue |
| BackendTLSPolicy ← SecretRefManager | Secret | cascading requeue | Secret changes automatically re-process the policy |

## Actual material field binding

`refs::backend_tls_fields::BackendTlsRefs` borrows the actual policy CA ordinal
and client option. Native CA group/kind remain required; only the empty core
group is supported. The client option retains its trimmed same-namespace DNS
name grammar. The shared native eight-reference bound precedes reference
preparation, acquisition and status traversal. Partial status checks material
cardinality before matching; complete Gateway publication requires every typed
reference/material pair, with no surplus material. Same-name Secret and
ConfigMap references remain distinct through the existing ConfigMap provenance
marker. Controller status is readonly; the Gateway checks pairing at cold
publication and keeps peer construction, winner selection and pool identity
ownership unchanged.

An omitted owner namespace means `default`; target namespace is the policy's
effective local namespace. Default acquisition accepts one exact qualified or
bare cached source, rejects both present, and preserves actual source metadata.
Nondefault acquisition never reads bare keys. These two cache reads are not an
atomic snapshot: ordinary typed dependencies precede acquisition, and source
events requeue the policy for reconciliation. Bare-key semantic notifications
are limited to Gateway and BackendTLSPolicy; raw-key observers remain intact.

## Full-instant arbitration transport (ARCH-055)

Resources owns `common::policy_precedence::policy_precedence_key`: complete UTC
instants sort oldest first, present timestamps precede missing timestamps, and
existing lexical identities break exact ties. Controller uses raw resource keys;
Gateway uses resource names within its existing target buckets. Identity and
namespace normalization are unchanged.

Every parse overwrites `spec.preparedPrecedence` from source metadata. Its optional
`creationTimestamp` uses chrono serde, preserving fractions and the accepted date
range across conf-sync even though Kubernetes metadata Time serializes seconds.
An empty prepared object denotes a legitimately absent source timestamp; absent
preparation is invalid on Gateway and preserves invalid exact/whole occupancy.
The carrier is serialized but hidden from the operator schema. Status sidecar
seeding replaces only typed policy status, preserving the source instant and
condition transition history. New Gateways require refreshed prepared output;
upgrade Controller/Gateway together and reprocess configuration.

## Final ancestor capacity and serving admission (ARCH-106)

The Controller composes the final ancestor list against the observed status,
including every foreign-controller entry and same-controller entry without
current Gateway ownership proof. The shared cold composer reserves existing
canonical owned keys before sorted new keys and admits at most 16 total entries.
Discovery itself is not truncated: a lexical newcomer cannot displace a
represented Gateway. An overfull protected observation is retained, with status
persistence suppressed; it never authorizes a write exceeding the schema limit.

The schema-hidden `preparedAncestorAdmission` carrier records the admitted
Gateway keys for Kubernetes or explicit unrestricted admission for native
FS/etcd configurations. Parsing replaces submitted values. Missing or malformed
preparation fails closed. This is separate from policy eligibility: the actual
winning exact/whole target policy remains selected even when its Gateway is
refused, so refusal cannot reveal a fallback policy. Shared Resources candidate
eligibility and cold arbitration keep Route diagnostics aligned with Gateway
selection, including invalid exact-section occupancy.

HTTP/gRPC, AI, direct Service endpoints, response-cache eligibility and all L4
selected-backend resolution paths check the winning policy against the matched
logical Gateway. The check borrows a bounded prepared set; it does not allocate,
format keys or acquire a lock. Weighted and named selection retain denied slots.
The five Route kinds report `ResolvedRefs=False` with `PolicyAncestorLimit` for
affected attachments; unrelated backends and represented Gateway attachments
remain usable. Admission-only changes rotate effective policy generations.

After policy publication or removal, changed preparation requeues dependents of
both old and new targets through their existing queues. Unchanged preparation
does not fan out. This ordering allows source-status-only slot release to restore
Route diagnostics and serving without a second queue or handler IO. HTTP/AI keep
their captured policy generation. L4 performs admission before discovery using
one read of each policy store, preserving its existing live discovery/degrade
behavior; separate stores are not one atomic snapshot.
