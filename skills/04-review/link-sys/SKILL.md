---
name: review-link-sys
description: Review knowledge for link-sys, grouped by decision type.
---

# link-sys review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [Best-effort log delivery during Gateway process exit](intentional-design/process-exit-log-delivery-best-effort.md)
- [Access-Log Fallback Sink + Manual Backfill — DESIGN-TRADEOFF (accepted)](intentional-design/access-log-fallback-backfill-tradeoffs.md)
- [LinkSys core/runtime module split — `link_sys_core` (common) vs `link_sys` (gateway) — FIXED](intentional-design/core-runtime-module-split-naming.md)
- [LinkSys health metric: free-function emit + per-link label — NOT-AN-ISSUE](intentional-design/health-metric-free-function-and-cardinality.md)
- [`DataSender::healthy()` intentional non-flips & disk-probe tradeoffs — DESIGN-TRADEOFF](intentional-design/healthy-semantics-intentional-non-flips.md)
- [Kafka `healthy` flag uses `Ordering::Relaxed` — NOT-AN-ISSUE](intentional-design/kafka-health-flag-relaxed-ordering.md)
- [Kafka broker dialing remains SDK-owned without in-process egress classification](intentional-design/kafka-broker-dialing-sdk-owned.md)
- [LinkSys store.rs dispatch functions — Extract-Function / Dedup — DESIGN-TRADEOFF](intentional-design/link-sys-dispatch-extract-function.md)
- [LinkSys webhook `webhook_call` Ok-arm — Extract-Function — DESIGN-TRADEOFF](intentional-design/webhook-call-extract-function.md)

## Regression guards

- [Full-set retain serializes with delete join replay](regression-guards/full-set-retain-serializes-join-replay.md)
- [A superseded LinkSys publication keeps the runner alive](regression-guards/persistent-runner-publish-supersession.md)
- [Kafka producer state has per-client aggregate bounds](regression-guards/kafka-aggregate-queue-bounds.md)
- [Redis configuration fields have one validated runtime meaning](regression-guards/redis-configuration-contract.md)
- [Redis DNS and egress policy is enforced at every Fred dial](regression-guards/redis-dial-time-egress-policy.md)
- [Redis pool health is aggregate and connection logs are transition-only](regression-guards/redis-pool-aggregate-health.md)
- [ResolvedTarget Has No Webhook Compatibility Alias — FIXED](regression-guards/resolved-target-has-no-webhook-compatibility-alias.md)
- [LinkSys outbound TLS enablement and shared loading](regression-guards/shared-outbound-tls-loading.md)
- [OTLP provider ownership and safe transport](regression-guards/otlp-provider-ownership-and-egress.md)
- [Webhook publication stays outside the generic LinkSys lifecycle](regression-guards/webhook-publication-outside-generic-lifecycle.md)

## Compatibility constraints

- [Fred `dns` feature carries an unused legacy Hickory resolver](compatibility-constraints/fred-dns-feature-hickory.md)
