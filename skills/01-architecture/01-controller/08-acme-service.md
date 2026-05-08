---
name: controller-acme
description: ACME certificate automation service — driven by the EdgionAcme resource, Let's Encrypt-compatible client, DNS provider implementations, Leader-only execution, Secret integration.
---

# ACME Certificate Automation Service

## Overview

The ACME service is an optional Controller component that automatically issues and renews TLS certificates via Let's Encrypt (or any compatible ACME server). The service is driven by the EdgionAcme custom resource — users only declare the domain names and validation method, and the system handles the full certificate lifecycle automatically.

Core components:

- **EdgionAcme resource** — declarative certificate requirements that define domain names, ACME server, and validation method
- **ACME protocol client** — Let's Encrypt-compatible ACME v2 protocol implementation
- **DNS providers** — multiple DNS provider implementations for domain validation

## Processing Flow

```
User creates/updates EdgionAcme
        │
        ▼
  Handler receives and processes the request
        │
        ▼
  ACME client initiates a challenge with the ACME server
        │
        ▼
  DNS provider performs domain validation (DNS-01)
  or HTTP-01 challenge validation
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
2. **Handler processing**: the EdgionAcme Handler parses the resource configuration, extracting the domain list, ACME server URL, and validation method
3. **ACME client interaction**: requests certificate issuance from the ACME server (e.g. Let's Encrypt) and obtains the challenge token
4. **Domain validation**: performs proof of domain ownership using the configured method (DNS-01 or HTTP-01)
5. **Certificate issuance**: once validation passes, the ACME server issues the certificate; the client downloads the certificate chain and private key
6. **Secret storage**: writes the issued certificate and private key into the corresponding Secret resource for use by Gateway TLS
7. **Status update**: updates the status of the EdgionAcme resource, recording the certificate expiration time, issuance state, and so on

## ACME Protocol Client

The ACME client implements the RFC 8555 protocol and is compatible with Let's Encrypt and other ACME v2 servers. Main responsibilities:

- Account registration and key management
- Order creation and challenge retrieval
- Challenge response submission and status polling
- Certificate download and parsing

## DNS Providers

The DNS-01 challenge requires adding a TXT record to the domain's DNS records to prove ownership. The system provides multiple DNS provider implementations, each wrapping the corresponding API calls:

- Create a `_acme-challenge.<domain>` TXT record
- Wait for DNS propagation
- Clean up the TXT record after validation

## Leader-only Constraint

The ACME service runs only on the Leader node. Reasons:

- **HTTP-01 challenge**: requires a single endpoint to respond to the ACME server's HTTP validation request; running on multiple instances would make the challenge response non-deterministic
- **Avoid duplicate issuance**: multiple instances simultaneously requesting from the ACME server would cause unnecessary duplicate operations and risk hitting rate limits
- **State consistency**: exclusive Leader processing ensures atomicity and consistency of the certificate-issuance flow

In HA mode, when the Leader switches over, the new Leader takes over the ACME service and any in-flight issuance flow restarts.

## Integration Mechanism

### Secret Management Integration

After issuance, the ACME service writes the certificate into a Secret resource. Changes to the Secret trigger reprocessing of dependent resources (such as a Gateway Listener) via the requeue mechanism, enabling automatic hot reload of certificates.

### Cascading Requeue

State changes of an EdgionAcme resource trigger reprocessing of related resources via RequeueChain:

- EdgionAcme issued successfully → Secret updated → Gateways/Listeners depending on that Secret are reprocessed
- Certificate about to expire → EdgionAcme re-enqueued → automatic renewal flow starts
