---
name: review-rate-limit
description: Review knowledge for rate-limit, grouped by decision type.
---

# rate-limit review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [BandwidthLimit Per-Response-Stream Scope — DOCUMENTED, NOT-AN-ISSUE](intentional-design/bandwidth-limit-per-stream.md)
- [RateLimitLocal Cluster-scope divisor is a per-controller connection count — ACCEPTED (ctrl-center-01)](intentional-design/cluster-scope-instance-count-divisor.md)
- [Deny-Response `shutdown()` Scope — ACL Plugins Shut Down, Rate-Limit Plugins Deliberately Do Not](intentional-design/deny-response-shutdown-scope.md)
- [Rate-limit response-header injection duplication — ACCEPTED TRADEOFF (plugins-http-F004, 2026-05-26)](intentional-design/header-injection-duplication.md)
- [Redis Rate Limiting Fail-Open — Accepted Tradeoff](intentional-design/redis-fail-open.md)

## Regression guards

- [Named Redis quotas and bounded single-response orchestration](regression-guards/redis-named-quotas.md)

- [GraphQL query-cost budgets and bounded parsing](regression-guards/graphql-query-cost-budgets.md)
- [Independent GraphQL selected-operation shape guard](regression-guards/graphql-guard-selected-shape.md)
- [Static GraphQL whole-document safelist](regression-guards/graphql-static-safelist.md)
- [Rate-limit response header names are validated at the resource boundary](regression-guards/response-header-name-validation.md)
- [Local allowance and Redis overflow admission](regression-guards/local-allowance-redis-overflow.md)

- [Local connection admission and configuration transitions](regression-guards/local-connection-admission-transitions.md)
- [Shared Redis script descriptors and cache-miss recovery](regression-guards/redis-script-cache-lifecycle.md)
- [ConnectionRateLimit counters must be scoped to the owning plugin entry — FIXED](regression-guards/connection-rate-limit-entry-scoped-buckets.md)
- [ConnectionRateLimit listener and route attachments are independent policy layers — ACCEPTED](regression-guards/connection-rate-limit-layered-attachments.md)
- [Rate-limit `default_key` fallback vs `OnKeyMiss::Allow` share a log tag — FIXED (m8-01)](regression-guards/default-key-vs-allow-log-tag.md)
- [Rate-limit key extraction shared helper — FIXED (plugins-http-F003, 2026-05-26)](regression-guards/key-extraction-shared-helper.md)
- [Multi-source rate-limit key composition must be injective — FIXED](regression-guards/multi-source-key-composition-injective.md)
- [Redis Token-Bucket Deny Branch Persists the Recomputed Balance — FIXED](regression-guards/token-bucket-deny-branch-persists-balance.md)
