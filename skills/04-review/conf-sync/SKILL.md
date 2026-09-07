---
name: review-conf-sync
description: Review knowledge for conf-sync, grouped by decision type.
---

# conf-sync review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [ConfCenter etcd Abstraction Boundary — Audited Design Tradeoff](intentional-design/conf-center-etcd-abstraction.md)
- [Generic `SimpleMapConfHandler<T>` Extraction — Declined](intentional-design/conf-handler-simple-map-pattern.md)
- [conf_sync File-Mode Cert Watch Keeps `(mtime, length)` — ACCEPTED-TRADEOFF](intentional-design/conf-sync-cert-watch-mtime-length-signature.md)
- [Cross-kind config updates are not atomic — HTTPRoute -> EdgionPlugins ExtensionRef torn-state window — ACCEPTED-TRADEOFF (bounded, self-healing, fail-closed; matches Envoy/Istio default baseline)](intentional-design/cross-kind-config-not-atomic-extensionref-torn-state.md)
- [No cross-kind dependency ordering — routes can be applied before their backends arrive — ACCEPTED-TRADEOFF (matches Envoy/Istio default baseline)](intentional-design/cross-kind-dependency-ordering-not-enforced.md)
- [No cross-resource-kind snapshot atomicity for the data-plane push (controller/push side) — ACCEPTED-TRADEOFF](intentional-design/cross-kind-push-no-snapshot-atomicity-controller-side.md)
- [No local config snapshot / warm-start on Gateway cold restart — ACCEPTED-TRADEOFF (matches Envoy/Istio default baseline)](intentional-design/gateway-no-local-config-snapshot.md)
- [No ACK/NACK feedback loop on the Watch stream — ACCEPTED-TRADEOFF (control-plane ingest validation replaces data-plane ACK/NACK)](intentional-design/no-ack-nack-control-plane-validation.md)
- [Fixed-capacity per-kind ring buffer — TooOldVersion forces a full relist — ACCEPTED-TRADEOFF (capacity is configurable; compaction is a large feature)](intentional-design/ring-buffer-fixed-capacity-toooldversion-relist.md)

## Regression guards

- [Transactional Controller channel selection](regression-guards/transactional-controller-channel-selection.md)
- [Generic Watch decode failure recovery](regression-guards/watch-decode-error-relist.md)
- [conf_sync Plaintext — Fail-Close (decided 2026-07-14)](regression-guards/conf-sync-plaintext-default-observable.md)
