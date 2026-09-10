---
name: redis-script-cache-lifecycle
description: Use when reviewing shared Redis rate-limit script initialization, cache misses, or execution retries.
knowledge_type: regression-guards
---

# Shared Redis script descriptors and cache-miss recovery

## Conclusion and scope

`RedisRateLimiter` references one process-wide immutable set of script descriptors.
Each descriptor pairs the exact Lua source with its Redis SHA1 identifier. The
constructor forces local initialization; operations do not preload remote scripts.

All five script families use the same executor: HTTP and Stream rate limiting,
cumulative AI quota, and credential reservation. Each operation tries `EVALSHA`
and falls back once to `EVAL` only when Fred reports an `Unknown` error whose first
ASCII-whitespace-delimited token is exactly `NOSCRIPT`. A timeout, disconnect,
authorization error, or arbitrary message containing that word is not permission
to replay a potentially completed operation.

## Rationale and evidence

The original per-instance async `OnceCell<ScriptHashes>` sequentially loaded all
five scripts before any operation. New plugin generations repeated these loads
even when the target Redis already held the selected script. This added five
remote commands to a fresh instance's successful single-policy cached path.

The corrected source is
`edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs`:
`ScriptDescriptor`, `SCRIPTS`, `RedisRateLimiter::new`, `eval_script`, and
`is_missing_script`. Lua bodies, keys, arguments, result parsing and caller failure
policies retain their existing contracts. The shared LinkSys pool still routes
each operation, including fallback, by its supplied keys.

This follows the canonical
[shared Redis executor](../../../01-architecture/00-common/05-unification-points.md).
The committed
[script lifecycle integration test](../../../../edgion-tests/tests/redis_script_lifecycle.rs)
provides executable coverage of the cache lifecycle contract.

The original change attributed its design precedent to a
`redis-rate-limit-practices` research report. That report is not included in this
repository; its external version comparisons are not independently verifiable
from the committed evidence and are not a correctness premise for this guard.

## Alternatives and limits

- A local descriptor is not a remote-cache presence flag. Restart, failover,
  script flushing, and client replacement can require another fallback.
- Concurrent cold operations may each receive `NOSCRIPT` and each execute their
  own operation through `EVAL`. No single-flight or exactly-once transport
  guarantee is implied.
- There is no added application retry after uncertain execution. Fred's existing
  transport behavior is unchanged.
- Script SHA1 is Redis's content identifier, not a cryptographic security claim.
- Single-policy Lua atomicity does not make several policies one transaction.
- This optimization did not fix D0100's local token-bucket race or HTTP CMS
  concurrency. The local race was subsequently corrected independently; see the
  [local admission guard](local-connection-admission-transitions.md).
- Preserve the [token-bucket deny-balance rule](token-bucket-deny-branch-persists-balance.md)
  and [existing failure policies](../intentional-design/redis-fail-open.md).

## Verification and re-evaluation

Verified on 2026-09-09:

- `cargo test -p edgion-gateway --lib rate_limit --locked`: 67 passed, including
  24 shared-core tests. Colocated coverage checks source/SHA pairing, descriptor
  sharing and cache-miss error classification.
- `cargo test -p edgion-tests --test redis_script_lifecycle --locked -- --ignored --nocapture`:
  1 passed on isolated Redis 7.4.11. The five script families used one `EVALSHA`
  when warm or one `EVALSHA` plus one `EVAL` when cold, without `SCRIPT LOAD`.
  Cache loss, replacement pools, persisted balances, three-algorithm weighted
  concurrency and warm/fallback error propagation passed.
- All five Lua bodies and existing integration-test files are unchanged.
- Existing ignored real-Redis `token_bucket_partial_refill_redis` and
  `credential_reservation_redis` tests both passed against the isolated instance.
- Format, workspace all-target check/Clippy, agent-document, SSA-force,
  metrics-inventory and Pingora-source guards passed. Clippy emitted repository
  warnings. Independent code/test review returned LGTM.

The new ignored test's global script-cache flush must never target a shared
fixture, even in a separate logical database. No full workspace test run or
latency/throughput benchmark was performed.

Standalone execution does not establish Cluster or failover behavior. Re-evaluate
when the Redis client error representation, Lua sources, script deployment model,
cluster routing, or retry policy changes.

## Resolution references

- `redis-rate-limit-practices` — user-approved shared script-lifecycle optimization,
  implemented and verified on 2026-09-09.
- `D0100` was corrected separately in the [local admission guard](local-connection-admission-transitions.md).
- `CAND-P2-001` — corrected the resource configuration module's stale `OnceCell`
  architecture comment to describe shared descriptors and exact `NOSCRIPT` fallback.
