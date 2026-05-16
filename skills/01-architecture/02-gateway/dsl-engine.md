---
name: gateway-dsl-engine
description: DSL engine architecture — data flow, source layout, plugin entry points, async/sync bridging, panic isolation, DslRuntime lifecycle. The architecture entry for DslPlugin maintainers; user-facing syntax / builtin reference lives in docs/.
---

# DSL engine architecture (DslPlugin)

> EdgionDSL is a lightweight request/response rule engine built on a custom bytecode VM, positioned for "zero runtime dependency, compile-time safety, sub-millisecond execution".
>
> This page is the architecture entry for engine maintainers. User-facing syntax, builtin function signatures, and runtime tuning live in `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/` — do not duplicate them here.

## 1. Data flow

```
                          Controller-side (or first-call on the gateway)
       ┌──────────────────────────────────────────────────────────────────────┐
       │                                                                      │
source │   Parser ──── AST ──── Compiler ──── Bytecode (JSON, base64)         │
text   │   (nom)      (≤2000   (type check,  Validator ↑                      │
       │   parse      AST       regex pre-   (≤1000 instr, ≤64 locals,        │
       │   budget     nodes,    compile,     ≤256 constants, ≤256 KB)         │
       │   50_000,    depth     constant                                      │
       │   ≤64 KB)    ≤64)      pool)                                         │
       └──────────────────────────────────────────────┬───────────────────────┘
                                                      ↓
session ─► DslPlugin::run_request / run_upstream_response_filter
                                                      ↓
                                       Vm.execute(session, log, runtime)
                                       async stack VM, per-instruction:
                                         steps ≤10_000  loop iters ≤100
                                         calls ≤500     stack ≤128
                                         string ≤8 KB   list ≤1024
                                         map ≤512
                                       wrapped by execute_safe → catch_unwind
```

Each stage rejects oversized or malformed input independently. Pre-compiled bytecode coming from the controller is re-validated by `Validator` after deserialization, so the data plane never trusts the controller's compile output blindly — see `DslPlugin::resolve_script` for the order: deserialize → `Validator::validate` → use.

## 2. Source layout

| Stage / role | File |
|---|---|
| Parser (nom combinators, parse budget 50_000, source ≤64 KB) | `src/core/gateway/plugins/http/dsl/lang/parser/` |
| AST node types | `lang/ast.rs` |
| Compiler (type check, regex pre-compile, constant pool, max regex pattern length 1024 B) | `lang/compiler/` |
| Compile-time entry that ties parser + compiler together | `lang/validator.rs` — `compile_dsl_source()` |
| Bytecode format + `BYTECODE_VERSION` + (de)serialization (gated by `MAX_DECODED_BYTECODE_LEN = 256 KB`) | `lang/bytecode.rs` |
| Standalone validator (also re-runs on deserialized bytecode) | `lang/validator.rs` — `Validator` |
| VM (`async fn execute`, panic-isolated `execute_safe`, `VmLimits`) | `lang/vm.rs` |
| Builtin implementations (`call_builtin`, `call_builtin_http`, `call_builtin_redis`) | `lang/builtins.rs` |
| Value enum (`Value::Str(Arc<str>)`, `List`, `Map(Box<IndexMap<String, Value>>)`) | `lang/value.rs` |
| Runtime errors | `lang/error.rs` |
| Plugin entry (RequestFilter + UpstreamResponseFilter impls, config-to-plugin factory) | `plugin.rs` |
| Per-plugin external-call context (limits + redis link table) | `runtime.rs` — `DslRuntime` |
| Static config schema | `config.rs` — `DslConfig` |
| Unit & integration tests | `tests/` |

## 3. Plugin entry points

`DslPlugin` implements both `RequestFilter` (RequestFilter phase, async) and `UpstreamResponseFilter` (UpstreamResponseFilter phase, sync) — built via `from_config()` and `from_config_response()` respectively from the same `DslConfig` (see `plugin.rs::resolve_parts`).

Both factories call `resolve_parts()`, which:

1. Calls `resolve_script()` — prefers pre-compiled `bytecode` over inline `source`. Source is compiled via `compile_dsl_source()`. Bytecode is re-validated through `Validator` after `CompiledScript::deserialize_base64`.
2. Calls `DslConfig::validate()` for static config sanity (limits, errorPolicy enum, etc.).
3. Calls `check_redis_link_names()` — a best-effort bytecode scan that rejects `redis.*` calls whose first-arg link name is not in `linkSysRefs`. False-positive / false-negative limitations are documented inline in `plugin.rs`; runtime `ok=false` is the fallback.
4. Builds `VmLimits` and `DslRuntime` from the config and constructs the `Vm`.

The same `DslPlugin` instance is shared across requests through `PluginRuntime` — only `&self` accessors run on the hot path. Per-request mutable state lives in `Vm::execute`'s local `VmState`, not in `DslPlugin`.

## 4. Async / sync bridging

`Vm::execute` is `async fn` (so it can await `http.call` and `redis.*` builtins). It must run from both an async filter phase (RequestFilter) and a sync one (UpstreamResponseFilter):

| Phase | Trait method | Bridge |
|---|---|---|
| RequestFilter | `async fn run_request(...)` | direct `.await` on `execute_safe(...)` |
| UpstreamResponseFilter | `fn run_upstream_response_filter(...)` (sync) | `tokio::task::block_in_place(|| Handle::current().block_on(execute_safe(...)))` |

`block_in_place` is required: Pingora calls the sync filter from inside the runtime worker thread, so a naive `block_on` would deadlock the worker. `block_in_place` lets Tokio move the current task aside and keep scheduling other tasks during the blocking call.

`UpstreamResponseBodyFilter` has no DSL entry point on purpose — that phase fires once per chunk, and the body-phase hot-path rules in [`05-plugin-system.md`](05-plugin-system.md) (the "no PluginLog in body phase" constraint) forbid per-chunk allocation. Running the VM there would violate them outright.

## 5. Panic isolation

`vm::execute_safe` wraps `Vm::execute` in `std::panic::AssertUnwindSafe(...).catch_unwind().await`. If the future panics:

- The plugin returns the configured `errorPolicy` outcome (`Ignore` / `Deny` / `DenyWith`).
- The request is not aborted; subsequent plugins still run, subject to the same policy.
- The panic does not propagate into Pingora's worker — Pingora's request loop does not tolerate an unwinding panic on the hot path.

This pairs with the runtime limits in `VmLimits` (step / loop / call / stack / string / list / map): the limits are the first line of defence — they raise typed `RuntimeError`s through normal control flow — and `catch_unwind` only catches the "this should never happen" tail. Both layers must stay in place; removing the limits would shift defensive load onto `catch_unwind`, which is more expensive and harder to reason about.

## 6. `DslRuntime` lifecycle

`DslRuntime` is **constructed per-plugin-instance at config-resolve time**, not per-request and not lazily — see `plugin.rs::resolve_parts`. It is `Send + Sync` and stored by value in `DslPlugin` (`self.runtime: DslRuntime`), so the hot path is a simple `&self.runtime` borrow.

```rust
// src/core/gateway/plugins/http/dsl/runtime.rs
pub struct DslRuntime {
    pub max_timeout_ms: u64,            // http.call wall-clock cap
    pub max_response_body_bytes: usize, // http.call body cap
    pub block_private_ips: bool,        // http.call SSRF guard for IP literals
    pub redis_links: HashMap<String, String>, // alias → "namespace/name" for get_redis_client
}
```

Shared resources are global, not per-`DslRuntime`:

- `dsl_http_client() -> &'static reqwest::Client` is a `OnceLock<reqwest::Client>` initialized on first use with `pool_max_idle_per_host(4)` and redirect policy `limited(10)`. Per-call timeouts come from `DslRuntime`, not from the client.
- Redis clients are resolved per-call from `link_sys` via `get_redis_client()` and the alias map; this matches the `rate_limit_redis` plugin pattern (ArcSwap read).

Implication: if the plugin's config changes (e.g. `httpMaxTimeoutMs` is updated), the controller emits a new `EdgionPlugins` entry; the gateway rebuilds `PluginRuntime` during preparse and constructs a fresh `DslPlugin` — and therefore a fresh `DslRuntime`. There is no in-place mutation path.

## Cross-references

- User-facing syntax / builtin reference / sandbox-limits user view: `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/` (`README.md`, `language-syntax.md`, `api-reference.md`, `sandbox-security.md`, `bytecode.md`, `examples.md`)
- Per-builtin implementation guides: `skills/03-coding/dsl/` (`http-call.md`, `redis-builtin.md`, `builtins-path.md`)
- `Value::Str` / `Value::List` design decision (why `Arc<str>`, not SSO): `skills/04-review/dsl-value-str/sso-migration-not-worth-it.md`
- Surrounding plugin framework (4-phase model, conditional execution, body-phase constraints): `skills/01-architecture/02-gateway/05-plugin-system.md`
