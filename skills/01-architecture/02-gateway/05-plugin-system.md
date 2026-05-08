---
name: gateway-plugin-system
description: Plugin system architecture — 4-phase execution, PluginRuntime pre-build, conditional execution, 29 HTTP plugins, Stream plugins.
---

# Plugin system

> The plugin system is Edgion Gateway's request-handling extension mechanism.
> Four execution phases cover the full lifecycle of a request; the PluginRuntime is built once during route preparse to avoid per-request overhead.

## Four-phase execution model

| Phase | trait | Timing | Async/sync | Method signature | Return |
|-------|-------|--------|------------|------------------|--------|
| RequestFilter | `RequestFilter` | Before forwarding to upstream | async | `run_request(&self, session, log) -> PluginRunningResult` | Can terminate the request or return a custom response |
| UpstreamResponseFilter | `UpstreamResponseFilter` | After upstream response headers received | sync | `run_upstream_response_filter(&self, session, log) -> PluginRunningResult` | Can modify response headers, terminate |
| UpstreamResponseBodyFilter | `UpstreamResponseBodyFilter` | Each response body chunk | sync | `run_upstream_response_body_filter(&self, body, end_of_stream, session) -> Option<Duration>` | Returns Duration for bandwidth limiting; **note: the signature deliberately omits `&mut PluginLog`** — see the "no PluginLog in body phase" constraint below |
| UpstreamResponse | `UpstreamResponse` | After the upstream response completes | async | `run_upstream_response(&self, session, log) -> PluginRunningResult` | Async post-processing |

Execution flow notes:
- Within each phase, plugins run in registration order.
- If any plugin returns `ErrTerminateRequest` or `ErrResponse`, the phase terminates immediately and subsequent plugins are skipped.
- In the UpstreamResponseBodyFilter phase, when multiple plugins return Duration values, the maximum is used (the strictest limit takes effect).
- After each phase completes, plugin logs are recorded into `ctx.stage_logs`, including plugin name, elapsed time, and conditional-skip information.
- **Exception**: the UpstreamResponseBodyFilter phase **does not** write to `ctx.stage_logs` and does not push entries into `ctx.pending_edgion_plugins_logs`. This phase is invoked once per chunk; per-chunk logging would blow up log volume on long streams. See the "no PluginLog in body phase" constraint below.

## PluginRuntime

`PluginRuntime` is the core container for plugin execution; it holds plugin lists for the four phases:

```rust
pub struct PluginRuntime {
    request_plugins: Vec<Box<dyn RequestFilter>>,
    upstream_response_plugins: Vec<Box<dyn UpstreamResponseFilter>>,
    upstream_response_body_plugins: Vec<Box<dyn UpstreamResponseBodyFilter>>,
    upstream_response_async_plugins: Vec<Box<dyn UpstreamResponse>>,
}
```

**Build timing**: HTTPRoute / GRPCRoute preparse phase (not per-request). Entry points (Gateway API standard filters) are split by scope: rule level uses `from_httproute_filters_for_rule()` / `from_grpcroute_filters_for_rule()`, backend level uses `from_httproute_filters_for_backend()` / `from_grpcroute_filters_for_backend()`. The legacy `from_httproute_filters()` / `from_grpcroute_filters()` are `#[deprecated]` and forward to the new entries with rule scope by default. EdgionPlugins CRD entries are loaded via `add_from_request_filters()` / `add_from_upstream_response_filters()` / `add_from_upstream_response_body_filters()` / `add_from_upstream_responses()`, each returning `Vec<String>` of validation errors. `clone()` returns an empty instance (rebuilt during preparse).

## Conditional execution

Plugins defined in EdgionPlugins are automatically wrapped with a `Conditional*` wrapper to support conditional evaluation:

| Wrapper | Wrapped object |
|---------|----------------|
| `ConditionalRequestFilter` | `RequestFilter` |
| `ConditionalUpstreamResponseFilter` | `UpstreamResponseFilter` |
| `ConditionalUpstreamResponseBodyFilter` | `UpstreamResponseBodyFilter` |
| `ConditionalUpstreamResponse` | `UpstreamResponse` |

Condition model (`PluginConditions`):
- **skip** conditions: if any condition matches, skip plugin execution.
- **run** conditions: all conditions must match to execute the plugin; otherwise skip.

Condition kinds include:
- `KeyExist` — whether a key exists (e.g., header, query param).
- `KeyMatch` — exact / multi-value / regex match against a key value.
- `Include` — whether the key value is in a given set (supports values list and regex).
- `Exclude` — value not in the set (the inverse of Include).
- `TimeRange` — whether the current time falls within a given range.
- `Probability` — probabilistic execution (e.g., 10% sampling).

Condition evaluation has three paths:
- `evaluate_detail` (async): used by async phases (RequestFilter, UpstreamResponse), takes `&mut PluginLog`.
- `evaluate_detail_sync` (sync): used by sync phases that need to write `cond_skip` into the outer PluginLog (`ConditionalUpstreamResponseFilter`).
- `evaluate_sync` / `Condition::evaluate_sync_silent`: **body phase only**, no log, zero allocation. Internally goes through `KeyMatchCondition::evaluate_sync_silent` / `IncludeCondition::evaluate_sync_silent` / `ExcludeCondition::evaluate_sync_silent`. **Do NOT call `evaluate_detail_sync` in the body phase**; doing so reintroduces per-chunk PluginLog allocation.

Gateway API standard filters (added through the private `add_from_httproute_filters_inner`; the public entries are the scope-explicit `from_httproute_filters_for_rule` / `from_httproute_filters_for_backend` mentioned above) are not wrapped with conditions and always execute.

Skip log format: `"skip:keyExist,hdr:X-Internal"` or `"!run:include,method"`.

### ⚠️ Critical constraint: regex must be compiled during preparse

`KeyMatch`, `Include`, and `Exclude` support a `regex` field. The `compiled_regex` field is annotated with `#[serde(skip)]` and is `None` after deserialization. **`compile()` MUST be called during preparse to complete compilation**; otherwise every request that hits the fallback path will execute `Regex::new()`, causing significant CPU overhead.

**compile call chain** (one-time, triggered when resources are received via gRPC list/watch):

```
resource.pre_parse()                        # event_dispatch.rs
  → EdgionPlugins::preparse()              # edgion_plugins/mod.rs
      → compile_conditions()               # iterate all entries, call PluginConditions::compile()
          → Condition::compile()           # conditions/types.rs
              → KeyMatchCondition::compile() / IncludeCondition::compile() / ExcludeCondition::compile()
      → add_from_*()                       # plugin_runtime.rs (the cloned conditions are already compiled at this point)
```

**When adding a new condition kind that uses regex**: add a corresponding arm to `Condition::compile()`'s match; otherwise the new kind's regex will never be compiled.

### ⚠️ Critical constraint: no PluginLog in body phase (per-chunk path must allocate zero)

**Background**: the trait signature of `UpstreamResponseBodyFilter::run_upstream_response_body_filter` **deliberately omits** the `&mut PluginLog` parameter. The body phase fires once per chunk — for a long stream (gRPC server-streaming, large file downloads), this can mean hundreds to thousands of invocations per request. Any per-chunk log allocation accumulates at O(chunks × plugins) and has previously caused two bugs:

- `ExtensionRefFilter` called `start_edgion_plugins_log` in the body phase, pushing one entry per chunk into `ctx.pending_edgion_plugins_logs` without a stage drain — long streams leaked memory continuously (fixed, issue g2-001).
- `run_upstream_response_body_plugins` allocated a `PluginLog::new(filter.name())` for every filter on every chunk and immediately dropped it — pure wasted allocation, plus silent loss of `cond_skip` and other diagnostics (fixed, issue g2-004).

**Hard constraints to follow when designing new plugins, conditions, or wrappers**:

1. **Do not add `&mut PluginLog` to `UpstreamResponseBodyFilter::run_upstream_response_body_filter`**. The trait signature is the contract; it promises no per-chunk logging in this phase. If diagnostics are truly needed someday, follow #4.
2. **When implementing a new body-phase filter**, do not call `log.push*` or `log.set_*` inside the filter — there is no log; use `tracing::debug!` for dev logs.
3. **When adding a new body-phase wrapper** (similar to `ConditionalUpstreamResponseBodyFilter`), do not create temporary logs via `PluginLog::new(...)`. Condition evaluation must go through `PluginConditions::evaluate_sync` or the `Condition::evaluate_sync_silent` chain — not `evaluate_detail_sync`.
4. **For "body-phase summaries"** (e.g., how many times this request hit rate limiting): aggregate per-request, not push per-chunk. Add an aggregation slot in `EdgionHttpContext`, accumulate during the body phase, and emit once at the end of the request into `stage_logs`. **Never** write per-chunk execution traces into the access log.
5. **ExtensionRef body path**: use `ExtensionRefFilter::resolve_runtime_silent`, **not** `prepare_execution` (the latter calls `start_edgion_plugins_log`).
6. **Resource / configuration error diagnostics** ("referenced EdgionPlugins not found", "wrong kind", etc.) should be surfaced in Controller preparse / status conditions; **do not** rely on body-phase runtime logs as a backstop — body phase silently no-ops, and per-chunk error logs are themselves a DoS risk.

**Self-checklist** (run through this for every body-phase change during PR review):

- [ ] No `&mut PluginLog` added to the `UpstreamResponseBodyFilter` trait or its impls.
- [ ] No heap allocations on the body-phase hot path (`PluginLog::new(...)`, `String::from(...)`, `format!(...)`, `Vec::new()`, etc.).
- [ ] Condition evaluation uses the silent chain (grep for `evaluate_sync_silent` / `evaluate_sync` rather than `evaluate_detail_sync`).
- [ ] No `ctx.pending_edgion_plugins_logs.push` or `session.start_edgion_plugins_log` calls on the body path.

## HTTP plugin list (29 total)

### Authentication (8)

| Plugin | Module | Description |
|--------|--------|-------------|
| BasicAuth | `basic_auth` | HTTP Basic auth |
| JwtAuth | `jwt_auth` | JWT token verification |
| KeyAuth | `key_auth` | API key authentication (header/query) |
| LdapAuth | `ldap_auth` | LDAP directory authentication |
| HmacAuth | `hmac_auth` | HMAC signature verification |
| HeaderCertAuth | `header_cert_auth` | Authentication based on certificate info in request headers |
| OpenidConnect | `openid_connect` | OpenID Connect authentication |
| ForwardAuth | `forward_auth` | Forward auth requests to an external service |

### Security (3)

| Plugin | Module | Description |
|--------|--------|-------------|
| Cors | `cors` | CORS cross-origin resource sharing policy |
| Csrf | `csrf` | CSRF token verification |
| IpRestriction | `ip_restriction` | IP allow/deny lists |

### Traffic control (3)

| Plugin | Module | Description |
|--------|--------|-------------|
| RateLimit | `rate_limit` | Local in-memory rate limiting |
| RateLimitRedis | `rate_limit_redis` | Distributed rate limiting based on Redis (Lua scripts) |
| BandwidthLimit | `bandwidth_limit` | Bandwidth limiting (UpstreamResponseBodyFilter phase) |

### Request / response transformation (3)

| Plugin | Module | Description |
|--------|--------|-------------|
| ProxyRewrite | `proxy_rewrite` | Proxy path / Host / header rewriting |
| ResponseRewrite | `response_rewrite` | Response header modification (UpstreamResponseFilter phase) |
| RealIp | `real_ip` | Extract the real client IP from headers such as X-Forwarded-For |

### Routing (4)

| Plugin | Module | Description |
|--------|--------|-------------|
| DynamicExternalUpstream | `dynamic_external_upstream` | Dynamic external upstream (specify external address at runtime) |
| DynamicInternalUpstream | `dynamic_internal_upstream` | Dynamic internal upstream (select K8s Service at runtime) |
| DirectEndpoint | `direct_endpoint` | Specify a backend endpoint address directly |
| RegionRoute | `region_route` | Multi-region traffic routing: references a ServiceRegionRoute PluginMetaData (`serviceRegionRouteRef`) → clusterRef → ClusterRegionRoute PluginMetaData. The Gateway queries PluginMetaDataGatewayStore at runtime; uses ordered routeRules (RouteByDirectRegion / RouteByKeyConfMatch / RouteByHashRange) + hashKeyGet passthrough + hashCalc writeBack + failoverTo (static redirect). No baseInfo, no auto failover. |

### Other (8)

| Plugin | Module | Description |
|--------|--------|-------------|
| Mock | `mock` | Return a mocked response (no upstream forwarding) |
| DslPlugin | `dsl` | DSL script engine (Parser→AST→Compiler→Bytecode→VM, ~4700 lines; see the DSL engine section below) |
| CtxSet | `ctx_set` | Set request context variables |
| JweDecrypt | `jwe_decrypt` | Decrypt JWE-encrypted tokens |
| RequestMirrorPlugin | `request_mirror` | Request mirroring (clone the request to another backend) |
| RequestRestriction | `request_restriction` | Request restriction (by path, method, etc.) |
| AllEndpointStatus | `all_endpoint_status` | Return the status of all backend endpoints |
| DebugAccessLogToHeader | `debug_access_log` | Write access-log info to response headers (for debugging, UpstreamResponseFilter phase) |

## DSL script engine (DslPlugin)

> Source: `src/core/gateway/plugins/http/dsl/` (~4700 lines, including the `lang/` subdirectory).
> Enhancement design doc: `docs/superpowers/specs/2026-03-22-dsl-plugin-enhancement-design.md`.

EdgionDSL is a lightweight request/response rule engine built on a custom bytecode VM, positioned for "zero runtime dependency, compile-time safety, sub-millisecond execution".

### Architecture

```
source / YAML → Parser(nom) → AST → Compiler → Bytecode (JSON/base64) → VM execution
```

- **Parser**: nom combinators, parse budget of 50k calls.
- **AST**: up to 2000 nodes, nesting depth ≤ 64.
- **Compiler**: type checks, function-signature validation, compile-time regex validation; emits bytecode (≤ 256 KB).
- **VM**: synchronous stack VM, panics isolated via `catch_unwind`.

### Implemented capabilities (39 builtins)

| Category | API |
|----------|-----|
| Request read (15) | `req.header`/`method`/`path`/`query`/`query_string`/`cookie`/`client_ip`/`remote_ip`/`path_param`/`header_names`/`scheme`/`host`/`uri`/`content_type`/`has_header` |
| Request modify (6) | `req.set_header`/`append_header`/`remove_header`/`set_uri`/`set_host`/`set_method` |
| Response modify (3) | `resp.set_header`/`append_header`/`remove_header` |
| Context KV (3) | `ctx.get`/`set`/`remove` |
| Utility functions (12) | `log`/`len`/`substr`/`to_int`/`to_str`/`to_upper`/`to_lower`/`base64_encode`/`base64_decode`/`url_encode`/`url_decode`/`time_now` |

Language features: variables (let / let mut), if/else, while, for-range, for-in, logical / comparison / arithmetic operators, string methods (starts_with / ends_with / contains / matches), `return deny(status, body)` / `return next()`.

### Sandbox security model

| Layer | Limits |
|-------|--------|
| Parsing | source ≤ 64 KB, parse budget 50k, AST ≤ 2000 nodes, nesting ≤ 64 |
| Compilation | instructions ≤ 1000, locals ≤ 64, constants ≤ 256, regex syntax check |
| Runtime | steps ≤ 10k, loop ≤ 100, calls ≤ 500, stack ≤ 128, string ≤ 8 KB (all per-plugin configurable) |

### Pending enhancements (design completed)

Three steps (see the design doc for details):
1. **Step-1 (P0)**: implement 5 stub functions (sha256 / md5 / regex_find / regex_replace / range-as-expr); only `builtins.rs` is touched.
2. **Step-2 (P1)**: resp reads (resp.status / resp.header / resp.has_header) + break / continue.
3. **Step-3 (P2)**: implement the UpstreamResponseFilter trait so the response phase can execute scripts.

### Configuration

```yaml
type: Dsl
config:
  name: "my-script"
  source: |          # inline source (or use bytecode: precompiled)
    let auth = req.header("Authorization")
    if auth == nil {
      return deny(401, "Unauthorized")
    }
  errorPolicy: deny  # ignore | deny | denyWith
  vmLimits:          # optional, override defaults
    maxSteps: 50000
```

## Stream plugins

Stream plugins run at the TCP/UDP layer and use a two-stage architecture:

| Stage | Location | Timing | Plugins |
|-------|----------|--------|---------|
| Stage 1: ConnectionFilter | `connection_filter_bridge.rs` | pre-TLS, IP info only | `StreamIpRestriction` |
| Stage 2: TlsRoute | `tls_route/` | post-TLS handshake, post-route match | `TlsRouteIpRestriction` |

- `StreamPlugin` trait: defines the `run_connection_filter(ctx) -> StreamPluginResult` interface.
- `TlsRoutePlugin` trait: defines the TLS-route-level plugin interface.
- `StreamPluginRuntime`: manages the plugin chain for the ConnectionFilter stage.
- `TlsRoutePluginRuntime`: manages the plugin chain for the TLS-route stage.
- `StreamPluginStore`: receives EdgionPlugins resource changes via ConfHandler.

## Directory layout

```
src/core/gateway/plugins/
├── http/                          # HTTP plugin directory (28; with DebugAccessLogToHeader in gateway_api_filters/, total 29)
│   ├── basic_auth/                # one subdirectory per plugin
│   ├── jwt_auth/
│   ├── cors/
│   ├── dsl/                       # DSL engine (with lang/ subdirectory: AST, compiler, VM)
│   ├── ...
│   ├── common/                    # shared plugin utilities (auth_common, jwt_common)
│   ├── conf_handler_impl.rs       # EdgionPlugins ConfHandler implementation
│   └── plugin_store.rs            # HTTP plugin resource store
├── stream/                        # TCP/UDP layer plugins
│   ├── ip_restriction/            # Stream IP-restriction implementation
│   ├── tls_route/                 # TLS-route-level plugins
│   │   ├── ip_restriction.rs      # TLS-route IP restriction
│   │   ├── tls_route_plugin_runtime.rs
│   │   └── tls_route_plugin_trait.rs
│   ├── connection_filter_bridge.rs # Pingora ConnectionFilter bridge
│   ├── stream_plugin_runtime.rs   # stream plugin runtime
│   ├── stream_plugin_store.rs     # stream plugin resource store
│   └── stream_plugin_trait.rs     # StreamPlugin trait definition
└── runtime/                       # plugin framework
    ├── plugin_runtime.rs          # PluginRuntime core (4-phase management + plugin creation factory)
    ├── conditional_filter.rs      # conditional wrapper (4 Conditional* types)
    ├── conditions/                # condition evaluation engine (KeyExist, Include, Probability)
    ├── session_adapter.rs         # PingoraSessionAdapter (Pingora Session → PluginSession adapter)
    ├── log.rs                     # plugin execution log (PluginLog, StageLogs)
    ├── traits/                    # trait definitions for the 4 phases
    │   ├── request_filter.rs
    │   ├── upstream_response_filter.rs
    │   ├── upstream_response_body_filter.rs
    │   ├── upstream_response.rs
    │   └── session.rs             # PluginSession trait (unified session abstraction)
    └── gateway_api_filters/       # Gateway API standard filter implementations
        ├── request_header_modifier.rs
        ├── response_header_modifier.rs
        ├── request_redirect.rs
        ├── url_rewrite.rs
        ├── extension_ref.rs       # ExtensionRef (route/backend filter list referencing EdgionPlugins, Gateway API standard) — nested form removed; see docs/zh-CN/migrations/<version>-extension-ref.md
        ├── cors_filter.rs
        └── debug_access_log.rs
```
