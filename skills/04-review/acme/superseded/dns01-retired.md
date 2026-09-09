---
name: acme-dns01-retired
description: Historical DNS-01 findings after the user-authorized removal of built-in DNS providers.
knowledge_type: superseded
---

# DNS-01 implementation retired

On 2026-09-09 the user explicitly chose HTTP-01-only built-in ACME to remove
DNS provider dependencies and credential management. DNS-01 configuration,
Cloudflare/AliDNS/Pebble adapters, credential resolution, TXT propagation probes,
and DNS-specific tests were removed; the Route 53 implementation task was cancelled.
The current boundary is [ACME supported scope](../../../01-architecture/01-controller/08-acme-service.md#supported-scope).

## Historical evidence

The retired implementation previously needed guards for:

- Provider error redaction: normalized operation/error classes excluded credentials,
  response bodies, signed URLs, record IDs, domains, and challenge values.
- AliDNS cleanup: exact TXT ownership, bounded complete pagination before deletion,
  and validated HTTP/business acknowledgements including RecordId and RequestId.
  Cleanup retained the first error while attempting subsequent deletions.
- Propagation polling: authoritative TXT checks, confirmed-missing versus
  unverifiable timeout outcomes, and a real consumer for propagationCheckInterval.

These are historical corrections, not current behavior or instructions to restore
DNS-01. General redaction and bounded outbound-I/O rules still apply elsewhere.
DNS-specific test removal follows feature retirement; HTTP-01/account tests remain,
and unsupported challenge types and wildcard requests have rejection coverage.
Historical test runs do not constitute fresh verification of this removal.

## Searchable provenance

- acme-dns-provider-error-redaction
- acme-alidns-cleanup-response-validation
- acme-alidns-cleanup-record-selection
- acme-dns01-propagation-check-interval-dead-knob
- acme-dns01-propagation-polling

Re-evaluate only after an explicitly authorized product-scope change, not because
an old finding or integration inventory names the removed implementation.
