# Connection-rate Redis client snapshot scope

Status: deferred ownership and reload-semantics decision for the user.
Source baseline: `b35589a89`. No runtime change.

## Current evidence

ConnectionRateLimitPlugin::on_connection first consumes the local listener gate,
then optionally awaits the distributed listener dimension, then checks the source
IP dimension. A denial stops evaluation. Each reached redis_dimension resolves
its own client with get_redis_client, checks health, constructs its key lazily,
and awaits the canonical limiter under its own timeout.

When both Redis dimensions are configured and reached, one invocation can perform
two lookups for the same immutable redis_ref. This is not two lookups for every
connection: local denial does none, and single dimensions or earlier Redis denial
perform at most one. Separate listener/route attachments remain independent
invocations and must not be deduplicated.

Gateway link_sys/runtime/store.rs resolves the currently published runtime entry;
PersistentRunner::current uses ArcSwap load() and clones the optional client Arc.
The first dimension's await permits replacement, unpublication or delete/recreate
before the second lookup. If an unavailable first dimension fails open, a newly
published client can be observed by the second dimension.

Pinning one optional client for the invocation could avoid one runtime lookup,
snapshot read and client-handle acquisition in the two-dimension case. It also
changes those observations and retains the selected handle across dimensions.
An Arc keeps memory owned, not a retired generation operational: publication and
removal can retire/shut down a client while callers still hold it. Health must
still be evaluated at each dimension unless a separate behavior change is
explicitly approved. Retaining the runtime entry and reading current again also
needs delete/recreate analysis; it is not automatically equivalent to a fresh
store lookup.

No lookup/refcount bottleneck or net throughput/memory gain has been measured.
This is distinct from [redis-03](redis-03-pool-member-selection.md), which selects
Fred pool members within a captured generation, [redis-01](redis-01-compiled-policy-validation.md),
which concerns policy validation, and [connection-rate-01](connection-rate-01-local-gate-lookup-ownership.md),
which concerns local bucket locking and ownership.

## Decision

Keep the current per-dimension resolution, or explicitly select a per-invocation
client snapshot after local admission. Define unavailable-to-ready, replacement,
unpublication and delete/recreate behavior before implementation. Do not cache a
client for the plugin lifetime, hold a store guard across Redis, change the
persistent-runner publication mechanism, or introduce a second client owner.

Measure the existing lookup cost independently of Redis latency for zero, one
and two reached dimensions, including local exhaustion and known-down clients.
Compare retained handles and replacement behavior as well as lookup counts. Keep
the current implementation if the saving does not justify changed visibility.

## Acceptance for an approved implementation

Preserve local-before-Redis and listener-before-source order, first-denial exit,
local token consumption without a Redis-denial refund, independent entry-scoped
keys, and cumulative policy attachment layers. Preserve lazy key preparation after
availability/health checks, per-dimension failure policy and exact static denial
reasons. Existing bounded failure/rejection metrics retain their owners and
classification; add no hot-path logs or high-cardinality labels.

Preserve separate per-dimension Redis timeouts. Client lookup, health and key/
policy preparation currently precede the timed core future; do not silently turn
this into a combined timeout or include/exclude different work. Keep the shared
RedisRateLimiter, all runtime numeric/reply validation, exact NOSCRIPT-only
fallback and canonical client transport/reconnect behavior.

Use deterministic controlled handovers between dimensions to verify replacement,
absence/recovery, deletion/recreation, health changes and retirement with retained
handles. Exercise both failure policies, every dimension combination, local
rejection, first-dimension denial and timeout/cancellation. Assert which client
receives each command, not only final allow/deny. Current no-Redis fixtures and
runner tests do not prove the proposed composition under reload.

An implementation needs a frozen IMPL allow-list and independent reviews. Existing
integration fixtures remain unchanged; any live Redis validation uses an
explicitly owned test instance. Do not infer Cluster/failover guarantees from
unit tests or a standalone Redis result.

## Original sources and verification limits

- `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs`
- `edgion-gateway/src/link_sys/runtime/store.rs` and `runner.rs`
- [Local admission transitions](../../skills/04-review/rate-limit/regression-guards/local-connection-admission-transitions.md)
- [Layered attachments](../../skills/04-review/rate-limit/regression-guards/connection-rate-limit-layered-attachments.md)
- [Published health ownership](../../skills/04-review/link-sys/regression-guards/published-generation-health-transition-ownership.md)

This task records current source behavior and an unselected optimization. Retained
Arc behavior is also covered by the existing runner removal test; it is not
permission to bypass health or retirement. No candidate implementation or
performance result is claimed.
