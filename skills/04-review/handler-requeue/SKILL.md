---
name: review-handler-requeue
description: Review knowledge for handler-requeue, grouped by decision type.
---

# handler-requeue review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Handler Requeue Rule](mandatory-design/handler-requeue-rule.md)

## Intentional design

- [Class ownership requeues versus parameter propagation and listener reconstruction](intentional-design/controller-06-gatewayclass-egc-no-gateway-requeue.md)
- [Status-Write Failure Is Not Re-queued — DESIGN-TRADEOFF](intentional-design/status-write-no-retry-accepted.md)
- [Status IO Stays Inline in the Worker Path — DESIGN-TRADEOFF](intentional-design/worker-path-status-io-inline-accepted.md)

## Regression guards

- [Backend tls winner single snapshot](regression-guards/backend-tls-winner-single-snapshot.md)
- [Route→BTP Requeue Helpers Miss sectionName-Keyed Target Registrations — FIXED](regression-guards/btp-section-key-requeue-gap.md)
- [Controller-derived Fields Use the Processed Cache](regression-guards/controller-derived-fields-use-processed-cache.md)
- [BackendTLSPolicy Requeues Use the EBTP Typed Service Key](regression-guards/ebtp-typed-service-key-btp-requeue.md)
- [Merge-patch cannot clear skip-serialized status lists — FIXED (routes) / AUDITED (rest)](regression-guards/status-list-merge-patch-tombstone.md)
- [Register ReferenceGrant dependencies before authorization reads](regression-guards/reference-grant-dependency-before-authorization.md)
- [EdgionPlugins Wasm fetch/OCI Secret status derivation — FIXED](regression-guards/wasm-secret-refs-status-derivation.md)
