---
name: review-controller
description: Review knowledge for controller, grouped by decision type.
---

# controller review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Non-Kubernetes runtime events start after initialization](intentional-design/non-kubernetes-pre-init-runtime-events-unsupported.md)

## Regression guards

- [FS initialization delivery and backpressure](regression-guards/fs-initial-delivery-backpressure.md)

- [Structured file initialization envelopes](regression-guards/structured-file-init-envelope.md)

- [Configuration writer error has one export](regression-guards/conf-writer-error-single-export.md)
- [Reference-manager APIs use canonical names](regression-guards/reference-manager-canonical-surface.md)
- [TLSRoute canonical API ingress](regression-guards/tls-route-canonical-api-ingress.md)

- [BackendTLSPolicy canonical API ingress](regression-guards/backend-tls-policy-canonical-api-ingress.md)
- [Source-scoped watch snapshots and one-time bootstrap](regression-guards/source-scoped-watch-snapshots.md)
- [Initialization completion starts only one worker](regression-guards/single-worker-initialization.md)
- [Etcd resource keys are authoritative over content identity — FIXED](regression-guards/etcd-key-content-identity.md)
- [Leader-Job Call Sites — FIXED (helpers), three hardening proposals REJECTED](regression-guards/leader-jobs-call-site-consolidation.md)
- [Wedged Leader Never Restarts / Never Fails Over — FIXED (supervised loop + deadline + deadman + gauge)](regression-guards/liveness-probe-and-lifecycle-supervision.md)
