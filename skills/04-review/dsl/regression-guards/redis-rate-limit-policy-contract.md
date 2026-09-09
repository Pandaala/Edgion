---
name: redis-rate-limit-policy-contract
description: Use when reviewing DSL atomic Redis rate-limit policies, keys, numeric bounds, and result semantics.
knowledge_type: regression-guards
---

# DSL Redis rate-limit policy contract

## Conclusion and scope

The `dsl-redis-rate-limit` feature connects `redis.rate_limit(link, policy, key,
cost)` to the canonical Gateway `RedisRateLimiter`. A bare `redis.incr` remains
a cumulative counter, not a time-window limiter. The original task was a feature
request, with a genuine documentation defect in its cumulative-counter examples;
it was not a defect in Redis INCR itself.

`DslConfig.rate_limit_policies` owns named operator policies; `DslRateLimits`
resolves immutable algorithm/window/key-prefix/timeout values at construction.
The builtin resolves the current client from `DslRuntime.redis_links` on each
call. No owned client is frozen into a plugin generation.

## Rationale and evidence

Canonical authority: `skills/00-design-rules.md` sections D, U, and Z, the
resource schema/transport contract, and `skills/03-coding/dsl/redis-builtin.md`.
Source entry points are `edgion-dsl/src/{bytecode.rs,compiler/builtins.rs,script.rs}`,
`edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs`, and Gateway
`plugins/http/dsl/{runtime.rs,plugin.rs,lang/builtins.rs}`.

- Exactly four arguments are checked through the shared compiler arity catalogue,
  including deserialized bytecode. The existing conservative alias check remains
  best-effort; dynamic calls still require the runtime result-map error path.
- Policy identity length-delimits namespace and name and includes rate/window;
  the shared key builder contributes algorithm and window suffix. Independent
  namespaces cannot collide by delimiter composition. Identical explicit policy
  identity on the same Redis database deliberately shares counters. Changing
  algorithm/rate/window selects fresh state; changing only timeout preserves it.
- Calls use one existing Lua operation and the shared exact-NOSCRIPT fallback.
  See [cache lifecycle](../../rate-limit/regression-guards/redis-script-cache-lifecycle.md).
- `{ok:true,value:{allowed:false,...},error:nil}` is successful denial. Provider,
  argument and operation failures return a fixed error classification, never raw
  dependency details. VM errors remain distinct; examples set errorPolicy to deny.
- Input validation and result-capacity checks run before Redis I/O. Results need
  five map entries and up to 18 bytes for fixed error strings. Cost is a strict
  positive integer, and business keys over 4096 bytes are rejected, not truncated.

## Numeric domain and limits

`edgion-resources/src/redis_rate_limit.rs` owns the reusable input contract:
rate/cost 1..1000000, whole-millisecond window 100..2147483647, and nonnegative
timestamp at most 2^53-1. Rate/window is validated at configuration construction;
dynamic cost/time is validated immediately before execution. The Gateway quota
APIs import the same exact-integer constant; their distinct contracts are unchanged.

Under valid script-owned state, rate/cost times window stay below 2^51. Token
refill with elapsed below the window therefore has an exact integer product;
elapsed at least one window fills the bucket even if the finite intermediate
product rounds. This is an input/state/result bound, not a claim that every
intermediate is exact. Sliding-window weighting remains approximate.

Redis state is trusted and owned by these scripts. Numeric checks do not prove
correctness against external key corruption, synchronize Gateway wall clocks,
or remove the existing clock-rollback behavior. A timeout/cancellation may occur
after quota was charged. Existing LinkSys transport behavior applies; no new
application replay or exactly-once guarantee is introduced. Separate policies
are not a transaction. Retry delay is the existing algorithm's estimate; a cost
larger than rate can never pass.

The [numeric-domain task](../../../../tasks/todo/define-redis-rate-limiter-numeric-domain/define-redis-rate-limiter-numeric-domain.md)
still owns HTTP/Stream adoption, local-gate arithmetic and its stronger
all-intermediates-exact requirement. This feature does not close that work.

## Verification and re-evaluation

Fresh focused verification in this session:

- `cargo test -p edgion-dsl --lib redis_rate_limit --locked`: 1 passed.
- `cargo test -p edgion-resources --lib redis_rate_limit --locked`: 2 passed.
- `cargo test -p edgion-gateway --lib plugins::http::dsl --locked`: 334 passed.
- `cargo test -p edgion-tests --test dsl_redis_rate_limit --locked -- --ignored --nocapture`:
  1 passed against an exclusively owned Redis 7.4.11 container. Covered three
  algorithms, weighted concurrency, warm/cold command counts, script-cache loss,
  live client replacement, policy reload sharing/isolation, pre-I/O argument
  rejection, dependency errors and timeout without application replay.

- After the final Clippy-only changes,
  `cargo test -p edgion-gateway --lib redis_rate_limit --locked`: 26 passed,
  including shared-core tests and the new DSL adapter tests.
- `./cicd/checks/check_crd_coverage.sh`: 13 structural plus 8 condition tests passed;
  `cargo test -p edgion-resources --test plugin_crd_type_coverage --locked`: 6 passed.
- Fresh independent read-only review: LGTM, rechecked after the final style edits.
- Format, workspace all-target check/Clippy, agent-doc consistency, SSA force,
  metrics inventory, Pingora source policy and Gateway tracing guards passed.
  Clippy exits successfully with pre-existing warnings; none touch added lines.
  No full workspace test run was requested. The isolated Redis container was removed.

Source/direct-bytecode/alias tests are in `edgion-dsl/src/script.rs`;
configuration and domain tests are in the resource crate; Gateway VM tests are in
`plugins/http/dsl/tests/redis_builtins.rs`. The dedicated real-Redis test is
`edgion-tests/tests/dsl_redis_rate_limit.rs` and requires explicit isolated-server
ownership before flushing its script cache.

Re-evaluate on signature, policy identity, numeric domain, Lua algorithm, result
shape, transport retry, or client-publication changes. Historical cache lifecycle
tests do not substitute for fresh DSL invocation coverage.

## Resolution references

- `dsl-redis-rate-limit` — completed feature task, independently reviewed and
  verified in this session. The earlier INCR examples lacked window/expiry and
  are now explicitly cumulative-counter examples. The feature task is retained
  under the project task lifecycle; no review finding file existed to delete.
