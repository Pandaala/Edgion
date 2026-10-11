---
id: ARCH-051
status: candidate
kind: architecture-debt
priority: P3
partition: PART-192
analyst: /root/verify_ip_union
independent_verification: confirmed by /root/analyze_conversion
---

# Retire the orphan Webhook authentication parsing mechanism

The credential layer exports HmacResolver/HmacMaterial and
WebhookApiKeyResolver/WebhookApiKeyMaterial for the former WebhookAuth model,
separate from the active HTTP HMAC-auth plugin. Their only production-source
resolver calls belong to Controller AuthSpec for WebhookAuth. That implementation
itself says this type is not a LinkSys field and mints no owning token.
WebhookServiceConfig actually contains conn, request and resolved_secrets, with
no WebhookAuth field. Its Controller branch resolves request-template Secrets
and TLS instead. Common resolved_state returns NotRequired for Webhook; neither
ResolvedCredential::Hmac nor ::WebhookApiKey has a production construction site.
They persist as alternatives and impossible Elasticsearch fingerprint/auth
branches. Standalone schema/material tests do not establish a configured owner.

Current Webhook architecture and feature contracts explicitly replace the old
auth block with canonical request templates and resolved_secrets. Actual
ManagedOutboundClient::from_webhook consumes those fields from a bound LinkSys.
Current Controller success/missing-template-Secret tests and Gateway unresolved
Secret readiness tests exercise that live mechanism. Prepared-client publication
and retirement remain its active owners.

Retire both dedicated resolver modules/tests, their material types/exports,
WebhookAuth and its unused AuthSpec implementation, the two unproduced credential
variants and their Elasticsearch arms together. Remove only tests constructing
the retired mechanism. Preserve live shared material redaction, SecretSlot/auth
strictness, Webhook request/target validation, template Secret resolution and
readiness, TLS and prepared-client lifecycle tests. Update directly stale type
references without aliases, an authentication framework or another renderer.

This reduces a dead cross-crate branch family and authoritative credential
surface. It is maintainability debt, not authentication bypass, Secret leakage,
measured RSS/throughput or unbounded-allocation evidence. Enum arms alone do not
justify retirement: current field provenance and all actual production producers
jointly establish the absent entry. Original knowledge already identifies the
unused type and references tasks/todo/retire-unused-webhook-auth-type-and-schema.md,
absent from the current tree. Continue that debt lineage; historical HMAC upsert
and dispatch pages concern the former model and do not override current code.

Preserve all live Basic/Bearer/Elasticsearch/AI/ACME/TLS resolvers and their
provider policies. Parsing stays in Resources; acquisition/ReferenceGrant/status
remain Controller-owned. Retain independent Secret/transport/prepared/request/SDK
owners, current Zeroizing scope, redacted Debug, real conf_sync values, admin
adapters, schemars exclusions, and deny_unknown_fields on live auth structures.
Removed Webhook auth/defaultHeaders/requestMethod must still produce advisory
unknown-field feedback with lenient admission. Leave the shared CRD auth-schema
interior outside this bounded proposal because remaining provider uses require
separate inspection. Do not implement Webhook HMAC signing, relax readiness/TLS
proof, change retained-client behavior or live Elasticsearch fingerprint tags,
or add payload logs, traffic tracing, metrics or request convenience allocations.

Future verification should prove retired symbols/branches gone while actual
Webhook Secret transport, missing-material rejection, TLS proof, removed-field
feedback and all live provider authentication keep their contracts. Use existing
private test access and meaningful owner tests, without widened visibility.
Primary and personally inspected actual support hashes/scopes belong to
REPORT-PART-192.json. Static read-only review: no writes to production, builds,
tests, network, Git mutations, physical-erasure or incident claims. Independent
verification is pending.
