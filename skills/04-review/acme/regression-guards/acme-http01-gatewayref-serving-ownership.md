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

## Active attempt publication checks

The minimal correction for `acme-scheduler-trigger-admission` reuses the
eligibility gate before publishing certificate Secrets, EdgionTls, lifecycle
status, and active challenges (including clearing challenges). `AcmeAttempt`
retains references to the admitted resource and runtime plus the service
generation. A fresh GET must match the admitted UID/generation, have no deletion
timestamp, and pass current eligibility; the service generation is checked both
before and after the GET. A GET failure suppresses publication.

Lifecycle/challenge SSA retains forced field ownership and includes the checked
UID/resourceVersion, so an intervening change to that same resource rejects the
write. A retired attempt does not publish Failed or increment its retry budget
after failing the outer completion check; its processing marker is removed.
Status-only resourceVersion changes do not retire an otherwise eligible attempt.
The existing generation-scoped timer schedules a recheck after a rejected
completion check. `Issuing`/`Renewing` without a local processing marker is
interrupted work, subject to the same retry limit as `Failed`, rather than proof
that another local task is running. This prevents transient publication checks
from leaving a permanently in-progress phase. Renewal intent is retained.

This is deliberately a limited publication guard, not a cancellation framework
or exactly-once guarantee. Source validation and Secret/EdgionTls writes are
separate API operations, and serving ownership comes from the current local
runtime view. Handoff after validation, account credential writes, requests
already sent to the CA, and complete scheduler recovery remain outside this
correction. A rejected challenge cleanup may leave tokens until expiry or
subsequent reconciliation. Remaining work stays in the
[scheduler task](../../../../tasks/todo/acme-scheduler-trigger-admission.md).

Regression coverage is in `service.rs`:
`certificate_publication_rechecks_authority_between_writes` drives the actual
publication function with an in-memory Kubernetes transport, including changed
UID/generation, deletion, GET failure, stopped service, stop during GET, missing
serving authority, and changes between Secret/TLS/status writes.
`retired_attempt_cannot_publish_challenges_or_failure` checks both challenge
publication/clearing and failure status. No live CA or Kubernetes cluster is
claimed by these tests.
`interrupted_attempt_recovers_after_transient_publication_check` drives the
worker through transient GET failures, processing cleanup, the actual recheck
command, and resumed Issuing/Renewing work without invoking a CA.
Terminating resources are rejected before admission, preventing repeated
publication rejection while finalizers delay deletion. The publication transport
also exposed a pre-existing inferred EdgionTls plural mismatch, tracked
[separately](../../../../tasks/todo/acme-edgiontls-api-plural.md); its current path
assertion does not establish that the live API accepts that endpoint.

Fresh verification for this minimal correction: Controller `--lib acme` tests
passed (37 tests); independent read-only review returned LGTM after the
interrupted-attempt recovery and deletion-admission corrections. Workspace
check/Clippy, formatting, agent-doc validation, SSA force, metrics inventory,
Pingora source policy, and Gateway API transform checks passed. Clippy reported
only the two existing warnings in the unrelated DSL gRPC and Kafka tests.

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
