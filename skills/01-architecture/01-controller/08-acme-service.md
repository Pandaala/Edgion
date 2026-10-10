---
name: controller-acme
description: ACME certificate automation service — driven by the EdgionAcme resource, Let's Encrypt-compatible client, HTTP-01 validation, Leader-only execution, Secret integration.
---

# ACME Certificate Automation Service

## Overview

The ACME service is an optional Kubernetes-mode Controller component that automatically issues and renews TLS certificates through the configured ACME v2 endpoint. The EdgionAcme handler still validates resources in other backends, but the background issuance service is started only by the Kubernetes leader lifecycle.

Core components:

- **EdgionAcme resource** — declarative certificate requirements that define domain names, ACME server, and validation method
- **ACME protocol client** — Let's Encrypt-compatible ACME v2 protocol implementation
- **HTTP-01 challenges** — tokens published to Gateway through configuration sync

## Processing Flow

```
User creates/updates EdgionAcme
        │
        ▼
  Handler validates, resolves references/status, and notifies AcmeService
        │
        ▼
  AcmeService actor coalesces the trigger and reserves an attempt
        │
        ▼
  Gateway serves HTTP-01 challenge tokens
        │
        ▼
  Domain validation passes → certificate issued
        │
        ▼
  Certificate stored into a Secret resource
        │
        ▼
  EdgionAcme status is updated (with expiration time)
```

### Detailed Steps

1. **Resource intake**: the user creates or updates an EdgionAcme resource; it enters the Workqueue → ResourceProcessor pipeline
2. **Handler processing**: the EdgionAcme Handler validates configuration, resolves the account credential and `challenge.gatewayRef`, publishes Controller-owned HTTP listener attachments, and updates `Accepted`/`ResolvedRefs`. It compares UID/generation, ResolvedRefs, and the resolved listener proof with the previous processed cache, then notifies the service from `after_publish` when authority may have changed. Delete and authoritative denial also notify so an active attempt can retire. Status-only echoes do not reset the retry budget. See the [notification guard](../../04-review/acme/regression-guards/resource-change-notification-gating.md).
3. **Actor scheduler**: the Kubernetes leader's AcmeService owns one state entry per resource. It coalesces duplicate commands, keeps at most one active attempt, one pending trigger, and one earliest deadline per key, and runs Kubernetes/CA work outside the resource workqueue. Before CA work starts, it persists an attempt reservation in `status.scheduler`.
4. **Domain validation**: performs proof of domain ownership using HTTP-01
5. **Certificate issuance**: once validation passes, the ACME server issues the certificate; the client downloads the certificate chain and private key
6. **Secret storage**: writes the issued certificate and private key into the corresponding Secret resource for use by Gateway TLS
7. **Status update**: updates the status of the EdgionAcme resource, recording the certificate expiration time, issuance state, and so on

## ACME Protocol Client

The ACME client implements the RFC 8555 flow used by Let's Encrypt-compatible ACME v2 endpoints. Main responsibilities:

- Account registration and key management
- Order creation and challenge retrieval
- Challenge response submission and status polling
- Certificate download and parsing

## Supported scope

Built-in ACME supports HTTP-01 only; wildcard domains are rejected. DNS-01,
DNS provider adapters, and DNS webhook clients are outside the product scope.
Use cert-manager or another external issuer for DNS-01/wildcard certificates.
Edgion already consumes their standard TLS Secrets through Gateway/EdgionTls
certificate references and hot-reloads Secret updates; no DNS credentials or
provider dependencies belong in Edgion for this integration.

## Leader-only Constraint

The ACME service runs only on the Leader node. Reasons:

- **HTTP-01 challenge**: requires a single endpoint to respond to the ACME server's HTTP validation request; running on multiple instances would make the challenge response non-deterministic
- **Avoid duplicate issuance**: multiple instances simultaneously requesting from the ACME server would cause unnecessary duplicate operations and risk hitting rate limits
- **State consistency**: the actor serializes scheduling decisions per resource, while a live Kubernetes Lease fence rejects work from an obsolete leadership term

On leadership change, `stop_acme_service` cancels the actor, startup scan, and
active attempts. Each admitted attempt captures the service generation, source
UID/generation, serving proof, a unique attempt ID, and the current Lease UID,
holder, term, and transition count. The guard re-reads both the Lease and source
resource before every externally visible write or irreversible ACME operation.
Cancellation also interrupts long ACME polls and propagation waits.

This is fencing, not a multi-object transaction. A request already accepted by
the CA cannot be recalled, and source, Secret, EdgionTls, and status writes are
separate Kubernetes operations. Target writes use the target UID/resourceVersion
when it already exists, and every following operation revalidates authority.
After issuance, publication failures are retried with the certificate retained
in memory, so they do not create another CA order in the same service run.
Publishing that Secret can refresh the selected Gateway's proof. Once a
certificate is issued, resource notifications no longer cancel it blindly:
each write requires fresh converged eligibility and the same Gateway UID/spec
generation and listener identity/key set. Proof epoch and digest changes caused
by certificate reload are accepted; ownership denial, source replacement, and
actual selection changes still retire the attempt.
Transient Lease/source read failures wait inside the operation guard, retaining
the current order or registered account credentials until authority is known.

`status.scheduler` is the restart checkpoint. It records the source generation,
reserved attempt ID, consumed attempt count, start time, and next retry time.
Reservation is persisted before contacting the CA. A startup scan reconstructs
deadlines from this state; transient GET failures retain the actor entry and
retry, while a confirmed 404 removes it. A new spec generation or explicit Admin
trigger resets the budget. Ordinary resource/status notifications do not.
An interrupted final attempt is recovered as `Failed`; failure to persist that
terminal state schedules a control-plane retry without another CA request.

## Integration Mechanism

### Secret Management Integration

Account and EAB references follow Gateway API SecretObjectReference defaults:
group absent/empty means the core API group, kind absent means `Secret`, and
namespace absent means the EdgionAcme namespace. Other groups/kinds are unsupported.
Actual-owner AcmeRefs fields back one diagnostic adapter used in handler
validation/resolution, source eligibility, account restoration/registration and
EAB retrieval. Account and EAB have distinct loan types; only the account loan
can enter account persistence. Invalid targets report
`ResolvedRefs=False/InvalidKind` and cannot reach a credential Secret API call.
See the [target identity guard](../../04-review/acme/regression-guards/secret-reference-target-identity.md).

After issuance, the ACME service writes the certificate into a Secret resource. Changes to the Secret trigger reprocessing of dependent resources (such as a Gateway Listener) via the requeue mechanism, enabling automatic hot reload of certificates.

An existing account Secret is updated with its original UID/resourceVersion.
After a conflict, the service re-reads it and adopts valid credentials written
concurrently; an empty same-UID Secret can be retried from its fresh version.
A replacement UID or corrupt credentials are never overwritten.

### Cascading Requeue

State changes of an EdgionAcme resource trigger reprocessing of related resources via RequeueChain:

- EdgionAcme issued successfully → Secret updated → Gateways/Listeners depending on that Secret are reprocessed
- Certificate about to expire → the actor's per-key deadline becomes due → renewal flow starts

The renewal/retry queue is distinct from the resource Workqueue.

### HTTP-01 serving authority

`challenge.gatewayRef` authorizes only the selected plain-HTTP listeners. The
handler resolves `sectionName` and `port` against the Controller-published
Gateway, checks cross-namespace `ReferenceGrant`, and transports bounded
`ResolvedListenerAttachment` values over conf-sync. `allowedRoutes` does not
apply because EdgionAcme is not a Route.

Every service command, including startup, Admin, renewal, and retry work,
passes the same eligibility gate. The raw and processed objects must have the
same UID and generation, current `Accepted`/`ResolvedRefs` conditions, and an
attachment vector equal to a fresh live resolution. Cache publication skew is
retried by the same per-key deadline without consuming the issuance retry budget.
Resource changes cancel a pre-issuance attempt and leave one coalesced follow-up check;
after issuance, the retained certificate continues only under the publication
authority rules above.
Admin triggers take priority and reset the budget.

### Credential identity and operation authority

Secret API paths come from the actual field, and explicit returned name/namespace
must match before payload or target UID/resourceVersion is consumed. Account
persistence derives its destination from the retained attempt owner. Initial and
refreshed snapshots are checked before adopting concurrent credentials or issuing
CAS. These checks supplement the existing source/leader/attempt fences; they do
not grant authority or change the create/merge protocol and absent-target race.

The handler's account cache supports the finite default bare/qualified selector
and semantic Secret event recovery. That raw-storage projection does not apply
to API responses. EAB remains direct API retrieval under its existing semantics;
no account/EAB authorization parity or new Grant policy is claimed. Generated outputs use the separate finite output view described below.


### Actual generated destinations

The actual attempt owner supplies both certificate Secret and optional EdgionTls
destinations through the canonical ACME field adapter. All active identities are
validated before the first output read/write. Missing source namespace means
`default`; omitted storage namespace inherits it. Explicit empty/invalid values
fail. Enabled TLS uses its explicit name or `acme-{source name}`; disabled TLS
ignores its unused name. No missing source name becomes `unknown`.

EdgionTls remains in the source namespace and references the certificate in the
storage namespace. Its `autoEdgionTls.parentRefs` are copied independently of the
HTTP-01 challenge Gateway. Existing targets require explicit matching API name
and namespace before their optional UID/resourceVersion are reused. Forced SSA,
adoption, current-attempt/Lease and between-write checks remain independent.
This is not a multi-object transaction: a TLS response mismatch after successful
Secret publication prevents TLS/status writes but does not roll back the Secret.
