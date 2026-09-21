---
name: acme-http01-gatewayref-serving-ownership
description: Guard the Controller-resolved Gateway listener ownership contract for ACME HTTP-01 serving.
knowledge_type: regression-guards
---

# ACME HTTP-01 GatewayRef serving ownership

## Required invariant

An active HTTP-01 token is not global serving authority. The Gateway may return
its key authorization only when token, challenge domain, exclusive expiry,
physical `ListenerKey`, logical Gateway/listener identity, listener hostname,
and current `GatewayAttachmentProof` all match one immutable challenge
snapshot.

## Controller contract

`EdgionAcmeHandler` clears and recomputes
`spec.resolvedListenerAttachments`. This serialized internal field is
`#[schemars(skip)]` and must remain absent from the CRD. Resolution uses only a
Controller-published Gateway, accepts plain `HTTP` listeners, applies
`sectionName` and `port` together, ignores `allowedRoutes`, and checks the
canonical `ReferenceGrantStore` for cross-namespace references.

Secret, cross-namespace, and Gateway reverse dependencies are replaced on
apply and cleared on delete. Every Gateway publication epoch requeues
`EdgionAcme` proof consumers.

Every AcmeService command passes the processed-generation eligibility gate.
UID, generation, current `Accepted`/`ResolvedRefs`, resolved attachments, and a
fresh live resolution must agree. Publication skew uses one convergence timer
per key and service generation and does not consume issuance retries.

## Active attempt fencing

Every attempt carries a service generation, unique attempt ID, admitted source
UID/generation, serving proof, and Kubernetes Lease fence. The attempt guard
re-reads the Lease and source resource and reuses this eligibility gate before
ACME side effects, account persistence, certificate Secret/EdgionTls writes,
lifecycle status, and active challenge publication or clearing. Long waits and
CA polls also observe cooperative cancellation.

Lifecycle, challenge, and scheduler SSA include the checked source
UID/resourceVersion. Existing derived targets include their own UID and
resourceVersion. A ResourceChanged command cancels pre-issuance work after proof,
condition, generation, or identity changes. After issuance, each write accepts
a fresh proof only for the same Gateway UID/spec generation and selected
listener identities/keys; its digest may change when the issued Secret reloads.
The live eligibility gate still rejects ownership/condition denial and source
replacement. A stale attempt cannot clear a
successor's challenge state or publish terminal status into it. Rejected cleanup
may leave the stale token until its bounded expiry.

The complete scheduling, retry, restart, and Lease contract is recorded in the
[scheduler fencing guard](acme-scheduler-fencing-recovery.md). It remains a
fenced, at-least-once workflow rather than an exactly-once CA protocol or a
multi-object Kubernetes transaction.

Regression coverage is in `service.rs`:
`certificate_publication_rechecks_authority_between_writes` drives the actual
publication function with an in-memory Kubernetes transport, including changed
UID/generation, deletion, GET failure, stopped service, stop during GET, missing
serving authority, and changes between Secret/TLS/status writes.
`retired_attempt_cannot_publish_challenges_or_failure` checks both challenge
publication/clearing and failure status. No live CA or Kubernetes cluster is
claimed by these tests.
Terminating resources are rejected before admission, preventing repeated
publication rejection while finalizers delay deletion. The
`acme-edgiontls-api-plural` regression was caused by constructing the dynamic
API from a GVK that guessed `edgiontlses`. Publication now type-erases
`EdgionTls` so its declared `edgiontls` plural remains canonical, and the
transport test asserts that CRD-declared endpoint. This test verifies request
construction, not acceptance by a live Kubernetes API server.

Fresh verification evidence belongs to the task or change that modifies this
contract; historical pass counts are not proof of the current tree.

## Gateway contract

Challenge publication retains resource ownership and rebuilds one canonical
token snapshot for full and partial updates. A token has at most 64 distinct
candidates; the first excess candidate publishes `Overflow` and fails closed
in constant time. Multiple eligible candidates with different key
authorizations also fail closed. Removing or updating a resource rebuilds the
bucket, allowing overflow recovery. Compiling a resource with no resolved
HTTP attachments or no active challenges drops that resource's tokens even
when the other field still carries a leftover value.

The request path performs one challenge snapshot load, scans at most 64
candidates, and takes no blocking lock or tracing path.

## Serving revocation

`gatewayRef` is required on HTTP-01, so it cannot be omitted from the spec.
Revoking serving while a token is still in `status.activeChallenges` is done
by making the reference unresolvable (missing Gateway, no HTTP listener, or
denied cross-namespace grant). The handler clears
`spec.resolvedListenerAttachments` at the start of every parse and leaves it
unset when resolution fails; conf-sync then publishes that generation and the
Gateway store fails closed.

Operator Admin PUT is not a status writer. FileSystem and etcd merges keep
the stored `status`, matching Kubernetes spec replace. Empty or expired
`activeChallenges` in an apply YAML therefore do not revoke serving. Tests
must change `spec.challenge.gatewayRef` to drop attachments, or delete and
recreate the object so the first persisted status can carry an expired
challenge. Production clearing is `AcmeService` writing the `/status`
subresource.

## Re-evaluation triggers

- A new ACME challenge type or HTTPS-based serving mode is proposed.
- `ResolvedListenerAttachment`, `GatewayAttachmentProof`, or Gateway listener
  publication changes.
- Challenge tokens move to another snapshot/store or lookup moves after route
  selection.
- Cross-namespace reference policy or AcmeService command routing changes.

## Verification anchors

- `edgion-controller/.../handlers/edgion_acme.rs`
- `edgion-controller/.../handlers/route_utils.rs`
- `edgion-controller/src/services/acme/service.rs`
- `edgion-gateway/src/services/acme/challenge_store.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`
- Dynamic scenario `Gateway/AcmeGatewayRefOwnership/DynamicTest`: selected A,
  retarget B, unresolvable `gatewayRef` (`updates/remove`), then delete+create
  with `expireAt` in the past (`updates/expire`). The expired phase waits until
  the Gateway `configclient` snapshot shows `expireAt=1` before accepting
  dual 404, so post-delete emptiness cannot pass as expiry.
- D0026, 2026-09-14.
- `acme-gatewayref-removal-retains-serving-owner`, 2026-09-16.
