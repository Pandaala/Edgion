# DSL `redis.*` Builtins

Redis builtins use named `linkSysRefs` aliases and resolve the live LinkSys Redis client
on each call. Operations return `Map { ok, value, error }`, allowing the script to handle
provider-not-ready and Redis command failures without a runtime exception.

## Supported Operations

`get`, `set`, `del`, `exists`, `ttl`, `expire`, `incr`, `incr_by`, `hget`, `hset`,
`hdel`, `hgetall`, `hexists`, and `rate_limit` are registered in `BuiltinId` and the compiler builtin
catalogue. The first argument is always the alias from `linkSysRefs`.

## Ownership

| File | Responsibility |
|------|----------------|
| `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs` | `DslConfig.linksys_refs` schema and shared compile/validate entry |
| `edgion-dsl/src/bytecode.rs` | Redis `BuiltinId` variants and `BuiltinId::is_redis_builtin()` |
| `edgion-dsl/src/compiler/builtins.rs` | Namespace/method resolution and arity |
| `edgion-dsl/src/script.rs` | Best-effort static alias validation |
| `edgion-gateway/src/plugins/http/dsl/plugin.rs` | Build alias → `namespace/name` runtime map |
| `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` | Per-call client lookup and operation/result mapping |
| `edgion-gateway/src/link_sys/providers/redis/ops.rs` | Redis operations |

Gateway `dsl/config.rs` only re-exports the resource-owned config.

## Alias Resolution and Static Check

At plugin build, each configured alias maps to its full LinkSys key. The runtime builtin
then looks up the alias and resolves the current client through the LinkSys store on each
call; clients are not permanently captured by the plugin constructor.

The full LinkSys key is borrowed from the immutable alias map only for the synchronous
store lookup. Only the returned `Arc<RedisLinkClient>` crosses the command await; do not
clone the `namespace/name` string or retain a borrowed key across an await.

`check_redis_link_names` is deliberately best-effort. It validates an alias only when the
compiler can prove the call arguments are simple single-push opcodes and recover the
literal first argument. Calls with multi-instruction sibling expressions may skip the
static check and fail through the normal runtime result map. Therefore:

- a statically provable unknown alias rejects shared `DslConfig::compile_and_validate`;
- an unprovable/dynamic or skipped alias is not guaranteed to reject plugin load;
- runtime `ok=false` remains required and must not be treated as unreachable.

## Result Semantics

`redis.hgetall` uses the provider's `hgetall_bounded` entry: check raw entry
count and the fixed 1 MiB field-plus-value budget on a borrowed raw RESP frame
before owned-string conversion.
Overflow maps to `result_too_large`; successful entries retain per-string
truncation. redis-rs's complete reply aggregation precedes this check and remains
outside the bound. Dispatch `redis::cmd("HGETALL")` through `RedisLinkClient` and
the single-node `command_route` (key index 1). Do not replace this call with an
unbounded public `hgetall`.

Failures return a bounded error map, for example an undeclared alias, a declared link
whose provider is not ready, or a Redis operation error. The public `error` value is a
fixed class and never contains the alias, key, field, value, or dependency error. When
`maxStringLen` cannot hold the specific class, the value is the generic one-byte class
`e`; it is never a truncated prefix. Internal dependency detail is recorded only through
the bounded sensitive `PluginLog` path. Scripts should branch on `ok`; they may branch on
a specific class only when their configured string budget can hold it.

```dsl
// Configure the requests policy and set errorPolicy: deny; see the API reference.
let r = redis.rate_limit("ratelimit", "requests", req.remote_ip, 1)
if r["ok"] == false {
  log_warn("redis unavailable")
  return deny(503, "service unavailable")
}
if r["value"]["allowed"] == false {
  return deny(429, "rate limit exceeded")
}
```

For indexed Boolean fields, use `r["ok"] == false` and
`r["value"]["allowed"] == false`. Unary `!` binds before indexing in the
current grammar, so `!r["ok"]` does not negate the selected field.

## Atomic rate limiting

`DslConfig.rate_limit_policies` declares policies resolved into `DslRateLimits`
at plugin construction. `redis.rate_limit(link, policy, key, cost)` selects one
policy and resolves its live LinkSys client per call. It uses the canonical
`RedisRateLimiter::eval_policy`, with no additional Lua or uncertain-error retry.
The result envelope distinguishes successful denial from failure. Type/range
errors use fixed result-map classes; VM limits still use runtime errors.

The canonical numeric helpers live in `edgion-resources/src/redis_rate_limit.rs`.
They bound the DSL, HTTP and Stream input domains and the Gateway validates
request-rate replies before using them. They do not make sliding-window floating
weights exact or validate Redis state written by outsiders.
The operator namespace and length-delimited policy identity prevent accidental key
composition collisions. Equal explicit identity on the same database deliberately
shares counters. Configuration changes to algorithm/rate/interval select fresh keys.

See [the script API](../../../docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md#redis-rate_limit)
for configuration, bounds, timeout/cancellation and field semantics, and
[the review guard](../../04-review/dsl/regression-guards/redis-rate-limit-policy-contract.md)
for regression coverage. A bare `redis.incr` is only a cumulative counter; it never
establishes a time window or automatic expiration.

## Adding an Operation

1. Add the `BuiltinId` variant and include it in
   `BuiltinId::is_redis_builtin()` in `edgion-dsl/src/bytecode.rs`.
2. Add compiler namespace resolution and the argument-count contract.
3. Add the async implementation arm in Gateway `call_builtin_redis`.
4. Extend static validation only if the new call preserves the alias-first contract.
5. Add compiler/static-validation, VM/runtime result, provider-not-ready, and integration
   tests.

Integration fixtures live under
`edgion-tests/integration/conf/EdgionPlugins/Dsl/25_EdgionPlugins_dsl-redis.yaml` and
the EdgionPlugins DSL client suite.

Both request and async upstream-response resource compilation now borrow actual DSL
configuration through the existing prepared plugin topology before cache reuse
or alias-map construction. Intrinsically invalid source identity is
non-retainable; ordinary invalid scripts under a valid owner retain the existing
key-scoped policy. Alias and authored namespace grammar are unchanged, and
per-call typed Redis lookup remains live.

## Nontruncating GET

`redis.get_bounded(alias,key,max_bytes)` uses the same live per-call resolution,
GET ACL and redis-rs pool as other reads, with `redis::cmd("GET")` and the same
single-node `command_route`. Its positive usize-representable Int bound is admitted
before provider lookup; effective cap is min(requested,maxStringLen,65536). Borrowed
frame length and UTF-8 are checked before owned DSL conversion; oversize is
result_too_large and invalid reply is the new fixed invalid_response class.
redis-rs's full reply aggregation is outside this conversion bound. Missing is
successful nil; old get remains truncating. Classify `RedisError::is_timeout()`
before wrapping errors. The deadline is LinkSys `timeout.command` together with
caller cancellation, not a VM wall-clock timeout. Charge final Arc text before
copying. Keep all eight
RedisFailure tags and their small-string fallback lock-in tests exhaustive.
