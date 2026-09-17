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
