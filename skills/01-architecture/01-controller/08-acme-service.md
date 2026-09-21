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
  AcmeService mpsc worker processes an issuance command
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
2. **Handler processing**: the EdgionAcme Handler validates configuration, resolves the account credential and `challenge.gatewayRef`, publishes Controller-owned HTTP listener attachments, and updates `Accepted`/`ResolvedRefs`. It compares UID/generation and ResolvedRefs status/reason with the previous processed cache, then notifies the service from `after_publish` when that comparison changed and the attachment/account result permits notification. Status-only echoes cannot reset the service retry budget merely because a generic status CAS failed; the service sees the newly published eligibility conditions before consuming a recovery notification. See the [notification guard](../../04-review/acme/regression-guards/resource-change-notification-gating.md).
3. **Background worker**: the Kubernetes leader's AcmeService consumes its own `mpsc<AcmeCommand>` queue, verifies the raw object against the current processed generation and live Gateway attachment, then requests issuance from the ACME server
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
- **State consistency**: exclusive Leader processing ensures atomicity and consistency of the certificate-issuance flow

On leadership change, the old service sender is dropped and the new leader starts
its service plus startup scan. Detached timers and already spawned issuance tasks
are not synchronously aborted by `stop_acme_service`, so do not assume every
in-flight operation is cancelled and restarted.

Before certificate Secret, EdgionTls, lifecycle status, or challenge publication,
the active flow now rechecks its service generation, GETs the current EdgionAcme,
compares UID/generation, rejects deletion, and reuses the eligibility gate.
Status writes also carry the checked UID/resourceVersion. These are conservative
publication checks, not atomic fencing across the source resource, derived
resources, and leader election. Account registration and CA requests are not
cancelled; scheduler recovery remains outside this correction under the
`acme-scheduler-trigger-admission` follow-up.
An attempt rejected at completion releases its processing marker and schedules
an existing generation-scoped recheck. A persisted Issuing/Renewing phase without
a local processing marker is recoverable work, with the normal retry limit.

## Integration Mechanism

### Secret Management Integration

After issuance, the ACME service writes the certificate into a Secret resource. Changes to the Secret trigger reprocessing of dependent resources (such as a Gateway Listener) via the requeue mechanism, enabling automatic hot reload of certificates.

### Cascading Requeue

State changes of an EdgionAcme resource trigger reprocessing of related resources via RequeueChain:

- EdgionAcme issued successfully → Secret updated → Gateways/Listeners depending on that Secret are reprocessed
- Certificate about to expire → AcmeService timer sends `RenewalCheck` on its own command queue → renewal flow starts

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
retried by one generation-scoped timer per key without consuming the issuance
retry budget.
