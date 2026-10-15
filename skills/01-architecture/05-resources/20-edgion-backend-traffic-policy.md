---
name: resource-edgion-backend-traffic-policy
description: EdgionBackendTrafficPolicy typed Service and AI backend dependencies, conflict status, and Gateway arbitration.
---

# EdgionBackendTrafficPolicy architecture

EdgionBackendTrafficPolicy (EBTP) attaches to homogeneous same-Namespace `targetRefs` of either
core `Service` or `edgion.io/EdgionBackend`. It never targets HTTPRoute and never supplies
routing destinations. AiProxy selection still uses only `HTTPRoute.backendRefs`.

Typed identity is `(group, kind, namespace, name)`; Service and EdgionBackend objects with the
same Namespace/name remain distinct. The capability matrix is atomic:

| Target | Valid sections |
|--------|----------------|
| Core Service | All sections |
| AI EdgionBackend | `connection`, `retryConstraint`, `circuitBreaker`, `outlierDetection` |

AI targets reject `loadBalancer`, `healthCheck`, and `upstreamAuthority`. Provider selection is
route-scoped, provider health cannot be inferred through unauthenticated probes, and credential
outlier state is not a Service endpoint pool.

The Controller registers typed dependencies, validates target existence and capability, and
arbitrates conflicts by oldest creation timestamp then policy name. Apply/delete requeues only
affected policies through the EBTP workqueue.

For Service active checks, HTTPS and GRPCS own health-local TLS configuration.
The Controller clears and resolves CA Secret/ConfigMap and client Secret material
on every parse and registers unresolved references for requeue. References must
remain in the policy namespace. Resolved material is redacted in admin output
and excluded from the CRD schema. TLS validation/reference failures are separate
from target eligibility: the winning check remains attached and fails until its
own configuration recovers. See [active probe transport](../02-gateway/07-backends.md#transport-ownership).

The Gateway publishes one immutable ArcSwap snapshot keyed by typed target. AI requests capture the
complete effective-policy Arc for the selected backend at plan time. Core Service HTTP/gRPC
requests capture the same Arc before backend plugins and attempt side effects onto
`BackendContext.effective_policy`. Its `effective_policy_captured` latch pins absence
as well as presence; repeated preparation and later attempts cannot reacquire a
policy published after that decision. Direct callers capture at the start of
`begin_upstream_attempt` before effects when no earlier preparation occurred. Health-check
and outlier **status** stay live; only configuration (including the outlier handle Arc) is
captured. HTTPRoute retry configuration authorizes retries, while `retryConstraint` only limits
aggregate retry volume.

The captured retry-budget Arc owns a monotonic origin and epoch-tagged atomic
counter buckets. Budget and minimum-rate windows share one elapsed sample per
operation but retain their independent intervals. See
[retry-budget epochs](../../12-algorithms/retry-budget-epoch-buckets.md) for
publication, saturation, delayed-operation, and lock-free platform boundaries.

## Actual health TLS ownership

`HealthTlsRefs` borrows the actual EBTP active HTTPS/GRPCS field. Custom Secret/CA
contracts, same-namespace validation, bound material acquisition and the canonical
outbound TLS proof apply before cold transport construction. Native Gateway API
CA bounds and grammar are not inherited. Default-namespace file/etcd material may
be a unique bare or qualified source; both candidates fail closed as ambiguous.
Original material metadata stays intact, and ordinary typed dependencies drive
reconciliation after Secret/ConfigMap events. The two store reads are not atomic.

Health publication retains the existing winning EBTP Arc instead of copying its
active config. Active config/material/error equality plus source name, raw
namespace and UID control generation reuse; a source replacement rotates it.
The probe task borrows that owner and constructs TLS once at startup using
explicit empty defaults. Existing poll fencing prevents superseded tasks from
continuing. Invalid TLS keeps the winning check attached and failing; no sibling
policy or global/backend trust supplies a fallback.

## Full-instant arbitration transport (ARCH-055)

EBTP shares Resources' `common::policy_precedence` ordering and prepared carrier
with BackendTLSPolicy. Controller policy-only status seeding preserves source
metadata, and every parse replaces submitted `spec.preparedPrecedence` with the
complete source instant (including legitimate absence). The same instant updates
conflict state and gates sibling requeues. Chrono serialization retains fractions
across List/Watch independently of rounded Kubernetes metadata Time.

Gateway requires the outer prepared value in addition to complete target
eligibility. An empty prepared object is a valid missing timestamp, sorting last;
missing preparation withdraws eligibility without falling back to metadata.
Carrier equality participates in raw/prepared-owner reuse, so fraction-only
replacement rebuilds the existing old/new affected target union. Snapshot,
Service fan-out, AI limits and runtime-handle ownership remain unchanged. A new
Gateway requires fresh prepared Controller output after coordinated upgrade and
reprocessing; there is no legacy fallback. See the
[BackendTLSPolicy transport contract](12-backend-tls-policy.md#full-instant-arbitration-transport-arch-055).

## Final ancestor admission (ARCH-106)

EBTP uses the same final-history capacity composer and schema-hidden
`preparedAncestorAdmission` transport as BackendTLSPolicy. Protected history
consumes the 16-entry budget; existing owned keys retain priority, and only new
unrepresentable Gateway attachments are refused. Admission is checked after the
canonical winning policy is selected, so capacity never promotes a conflicted
candidate or denies unrelated backends. Missing preparation fails closed;
native FS/etcd preparation explicitly marks unrestricted admission.

Admission participates in effective-policy equality and generation reuse. HTTP,
gRPC and AI use their captured policy; cache eligibility checks admission before
lookup/publication; L4 checks before endpoint resolution while retaining existing
live application behavior. Change-gated fanout runs after publication/removal,
includes dropped targets and restores dependents after status-only slot release.
See the [shared capacity contract](12-backend-tls-policy.md#final-ancestor-capacity-and-serving-admission-arch-106)
for ownership, arbitration, bounded checks and cross-store limits.
