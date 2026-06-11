# DSL redis.* Builtins — Implementation Guide

## Overview

`redis.*` builtins let EdgionDSL scripts interact with pre-configured LinkSys Redis
connections via named aliases (declared in `linkSysRefs`). All functions are async,
never-panic, and never raise a runtime error. Each returns `Map { ok: Bool, value: <type>,
error: Str | Nil }` so the script author controls error handling.

The first argument to every redis.* function is a link alias string that refers to an entry
in the plugin's `linkSysRefs` list. This allows scripts to access multiple Redis instances.

## Supported functions

| Function | Args (incl. link) | Return value type |
|----------|-------------------|-------------------|
| `redis.get(link, key)` | 2 | `Str | Nil` |
| `redis.set(link, key, value [, ttl_ms])` | 3–4 | `Bool` |
| `redis.del(link, key)` | 2 | `Bool` |
| `redis.exists(link, key)` | 2 | `Bool` |
| `redis.ttl(link, key)` | 2 | `Int` (-1 = no expiry, -2 = not found) |
| `redis.expire(link, key, seconds)` | 3 | `Bool` |
| `redis.incr(link, key)` | 2 | `Int` |
| `redis.incr_by(link, key, delta)` | 3 | `Int` |
| `redis.hget(link, key, field)` | 3 | `Str | Nil` |
| `redis.hset(link, key, field, val)` | 4 | `Bool` |
| `redis.hdel(link, key, field)` | 3 | `Bool` |
| `redis.hgetall(link, key)` | 2 | `Map` (empty when key not found) |
| `redis.hexists(link, key, field)` | 3 | `Bool` |

## Key files

| File | Role |
|------|------|
| `src/core/gateway/plugins/http/dsl/lang/builtins.rs` | `call_builtin_redis`, `redis_ok`, `redis_err`, error helpers |
| `src/core/gateway/plugins/http/dsl/lang/bytecode.rs` | `BuiltinId` Redis variants |
| `src/core/gateway/plugins/http/dsl/lang/compiler/builtins.rs` | `resolve_namespace_method` + `builtin_expected_argc` entries |
| `src/core/gateway/plugins/http/dsl/lang/vm.rs` | `is_redis_builtin`, `CallBuiltin` dispatch |
| `src/core/gateway/plugins/http/dsl/runtime.rs` | `DslRuntime.redis_links: HashMap<String, String>` |
| `src/core/gateway/plugins/http/dsl/config.rs` | `DslConfig.linksys_refs: Vec<LinkSysRef>` |
| `src/core/gateway/plugins/http/dsl/plugin.rs` | Compile-time bytecode scan, client resolution via `get_redis_client()` |
| `edgion-resources/src/resources/edgion_plugins/plugin_configs/linksys_ref.rs` | `LinkSysRef { namespace, name }` type |
| `src/core/gateway/link_sys/providers/redis/ops.rs` | Actual Redis operations (`get`, `set`, `set_px`, `incr`, ...) |
| `src/core/gateway/link_sys/runtime/store.rs` | `get_redis_client("ns/name") -> Option<Arc<RedisLinkClient>>` |

## DslRuntime field

```rust
pub struct DslRuntime {
    // ... other fields
    pub redis_links: HashMap<String, String>,
}
```

Resolved at plugin-build time in `DslPlugin::resolve_parts()`:
```rust
let redis_links: HashMap<String, String> = config.linksys_refs.iter()
    .map(|r| (r.name.clone(), format!("{}/{}", r.namespace, r.name)))
    .collect();
```

Each entry maps a link alias (e.g., `"ratelimit"`) to its full namespace/name path
(e.g., `"edgion-default/ratelimit"`). At runtime, when a script calls `redis.get("ratelimit", key)`,
the first argument is looked up in this map and resolved to a client via the LinkSys store.

## Compile-time check

At plugin-load time, a bytecode scan checks that every redis.* function call references
a declared link alias. The scanner:

1. Iterates over all bytecode instructions.
2. When it finds a `CallBuiltin(redis.*, argc)` at position `i`, it looks back `argc` positions.
3. Expects a string literal (the link alias name) at `i - argc`.
4. Verifies that the alias exists in `config.linksys_refs[].name`.

If an alias in the script does not match any declared `linkSysRefs`, the plugin fails at load
time with a tracing::error log. No runtime error is produced.

## Error semantics

Redis builtins return `Map { ok: Bool, value: <type>, error: Str | Nil }`. When an operation fails:

- `ok=false, error="link 'x' not found in linkSysRefs"` — the link alias was not declared
  in the plugin's `linkSysRefs` list (should be caught at compile time).
- `ok=false, error="link 'x' not ready"` — the alias is declared but the LinkSys provider
  has not yet made a client available for this namespace/name pair.
- `ok=false, error="<redis error>"` — the Redis server returned an error (connection failure,
  command error, etc.).

Scripts can use patterns like `if !r["ok"] { log_warn(r["error"]); ... }` to handle failures.

## VM dispatch

In `vm.rs`:
```rust
OpCode::CallBuiltin(id, argc) => {
    state.tick_call()?;
    let result = if id == BuiltinId::HttpCall {
        self.call_builtin_http(...).await?
    } else if is_redis_builtin(id) {
        self.call_builtin_redis(id, argc, &mut state, runtime).await?
    } else {
        self.call_builtin(id, argc, &mut state, session, log)?
    };
    state.push(result, self.limits.max_stack_depth)?;
}
```

## Config YAML example

```yaml
spec:
  requestPlugins:
    - enable: true
      type: Dsl
      config:
        name: "rate-limit-dsl"
        source: |
          let r = redis.incr("ratelimit", "rate:" + req.client_ip)
          if !r["ok"] {
            log_warn("redis error: " + r["error"])
            return deny(503, "service unavailable")
          }
          if r["value"] > 100 {
            return deny(429, "rate limit exceeded")
          }
        linkSysRefs:
          - namespace: edgion-default
            name: ratelimit
        errorPolicy: deny
```

The script calls `redis.incr("ratelimit", ...)` where `"ratelimit"` is the alias
declared in `linkSysRefs[0].name`.

## Integration tests

Config: `examples/test/conf/EdgionPlugins/Dsl/25_EdgionPlugins_dsl-redis.yaml`
Routes: `examples/test/conf/EdgionPlugins/Dsl/HTTPRoute_dsl-redis.yaml`
Test client: `examples/code/client/suites/edgion_plugins/dsl/dsl.rs` — scenario 25 (4 test cases)

## Adding new redis.* functions

1. Add `BuiltinId::Redis<Name>` variant to `bytecode.rs`.
2. Add mapping in `resolve_namespace_method` in `compiler/builtins.rs`.
3. Add arity entry in `builtin_expected_argc` in `compiler/builtins.rs` (remember: arity includes the link alias as first arg).
4. Add implementation arm in `call_builtin_redis` in `builtins.rs`.
5. Add redirect arm in `call_builtin` (sync) in `builtins.rs` (returns `Internal` error).
6. Add to `is_redis_builtin` in `vm.rs`.
