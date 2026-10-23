---
name: redis-named-quotas
description: Review named Redis quota identities, bounded preparation, aggregate checks, result ownership, and partial debits.
knowledge_type: regression-guards
---

# Named Redis quotas

## Contract and ownership

RateLimitRedis uses one typed quotas list, with existing KeyGet identity sources,
constant cost and RateLimitPolicy windows per quota. Flat legacy quota fields are
rejected. Controller validation and Gateway construction share Resources checks:
1–16 quotas, 1–10 policies per quota, at most64 policies overall, unique bounded
names and a valid responseQuota. Ordinary multiple quotas require an explicit
nonzero timeout. Failure defaults remain ordinary Allow and GraphQL Deny.

Resources owns schema, HTTP preparation owns session key access, and the existing
protocol-independent RedisRateLimiter owns unchanged Lua algorithms. LinkSys is
the only client provider. Stream and AI consumers retain their own orchestration.

Key semantics remain omitted remoteIp, explicit-empty global and partial-missing
positional composites. Key-miss Allow skips only its quota. Prepare identities
before Redis. Bound retained keys with canonical UTF-8 truncation and release
oversized backing allocations; SmallVec inline1 avoids an orchestration heap
allocation for a single quota. Retained key payload is at most16*maxKeyLen.

Cold prefixes include resource/entry identity and an unambiguous named quota
segment. Names survive reordering; renaming creates new counters. Existing
policy-index and key-truncation semantics remain. Do not claim arbitrary-length
injectivity, cross-entry sharing or config-digest namespace isolation.

## Results, timeout and debit boundaries

Resolve one client Arc and sequentially check active quotas under one deadline.
Each quota retains existing all-policy core evaluation and within-quota result
merge. Stop at first denial or dependency error. Intermediate checks must not
queue HTTP headers or write a response. Render once outside the timed data-only
future; release prepared keys before response awaits. The missing/unhealthy-client
early branch also drops prepared jobs before entering its failure policy. Keys
and any spilled job storage are no longer needed there; key extraction, client
lookup, diagnostics and failure-policy ordering remain unchanged.

Successful headers use the selected checked quota, or first checked quota when
not configured. Explicitly selected skipped quotas emit no headers. Denial uses
only the denied quota's aggregate result. Never mix units across independent
quotas. Dependency failure emits no partial-success quota headers; existing
failure policy owns the outcome.

Individual Lua counters are atomic; multiple counters are not transactional.
Later rejection/failure retains earlier debits and timeout can leave uncertain
committed work. Do not retry/refund uncertain debits. Exact NOSCRIPT fallback and
single-key Cluster-compatible scripts remain unchanged.

## Evidence and re-evaluation

Coverage includes Resources validation, Gateway preparation/memory tests, the
shared live suite's tenant/user checks and counter reorder, and GraphQL root
units/static/dynamic scenarios. These historical coverage notes do not establish
fresh merged-tree verification. Re-review changed missing-source semantics, ownership across awaits,
response selection, defaults, namespaces, parallelism, client generation pinning,
Lua changes or transaction promises.

- [Current configuration contract](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md)
- [Resources schema](../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/rate_limit_redis.rs)
- [HTTP preparation/check/render](../../../../edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs)
- [Key composition contract](multi-source-key-composition-injective.md)
