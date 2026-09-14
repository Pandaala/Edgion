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
bucket, allowing overflow recovery.

The request path performs one challenge snapshot load, scans at most 64
candidates, and takes no blocking lock or tracing path.

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
- D0026, 2026-09-14.
