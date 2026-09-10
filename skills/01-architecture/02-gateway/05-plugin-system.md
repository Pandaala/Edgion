---
name: gateway-plugin-system
description: Plugin system architecture — 4-phase execution, PluginRuntime pre-build, conditional execution, 29 HTTP plugins, Stream plugins.
---

# Plugin system

> The plugin system is Edgion Gateway's request-handling extension mechanism.
> Four execution phases cover the full lifecycle of a request; the Gateway builds and shares plugin runtimes during configuration publication to avoid per-request construction.

## Four-phase execution model

| Phase | trait | Timing | Async/sync | Method signature | Return |
|-------|-------|--------|------------|------------------|--------|
| RequestFilter | `RequestFilter` | Before forwarding to upstream | async | `run_request(&self, session, log) -> PluginRunningResult` | Can terminate the request or return a custom response |
| UpstreamResponseFilter | `UpstreamResponseFilter` | After upstream response headers received | sync | `run_upstream_response_filter(&self, session, log) -> PluginRunningResult` | Can modify response headers, terminate |
| UpstreamResponse | `UpstreamResponse` | Final response headers, before downstream header commit and body streaming | async | `run_upstream_response(&self, session, log) -> PluginRunningResult` | Await header decisions, modify headers, or install bounded streaming companions |
| UpstreamResponseBodyFilter | `UpstreamResponseBodyFilter` | Data callbacks and terminal completion without trailers | sync | `run_upstream_response_body_filter(&self, body, end_of_stream, session) -> Option<Duration>` | Returns Duration for bandwidth limiting; **note: the signature deliberately omits `&mut PluginLog`** — see the "no PluginLog in body phase" constraint below |

The response stages have distinct boundaries. The Gateway's
`pg_upstream_response_filter` skips interim 1xx, arbitrates status retries,
normalizes the selected response, and runs synchronous `UpstreamResponseFilter`
plugins. `pg_response_filter` then runs asynchronous `UpstreamResponse` header
decisions before committing headers or forwarding the response body. Interim
1xx (such as 100/103) do not run either chain; 101 is a terminal upgrade response
and is not treated as interim.

Neither header stage exposes the complete main response body. The upstream body
remains streaming: body filters and bounded streaming processors run on its later
body path, while trailer companions run through the separate trailer dispatcher.
Do not implement full-response aggregation in `UpstreamResponse` or treat it as
an end-of-response hook. See the
[`UpstreamResponse` trait](../../../edgion-gateway/src/plugins/runtime/traits/upstream_response.rs)
and the [Gateway response driver](../../../edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs).

Execution flow notes:
- Within each phase, plugins run in registration order.
- If any plugin returns `ErrTerminateRequest` or `ErrResponse`, the phase terminates immediately and subsequent plugins are skipped.
- In the UpstreamResponseBodyFilter phase, when multiple plugins return Duration values, the maximum is used (the strictest limit takes effect).
- After each phase completes, plugin logs are recorded into `ctx.stage_logs`, including plugin name, elapsed time, and conditional-skip information.
- **Exception**: the UpstreamResponseBodyFilter phase **does not** write to `ctx.stage_logs` and does not push entries into `ctx.pending_edgion_plugins_logs`. This phase runs on data callbacks and no-trailers terminal completion; per-callback logging would blow up log volume on long streams. See the "no PluginLog in body phase" constraint below.

### Response body and trailer completion

The typed driver keeps these events distinct:

| Driver event | Streaming processors | Ordinary `UpstreamResponseBodyFilter` plugins | Trailer dispatch |
|--------------|----------------------|-----------------------------------------------|------------------|
| `Data { end_of_stream: false }` | Run on the mutable current chunk/window | Run on the immutable current chunk/window with `end_of_stream = false` | None |
| `Data { end_of_stream: true }` | Run | Run with `end_of_stream = true` | Dispatch `None` to `UpstreamResponseTrailer` companions |
| `TerminalWithoutTrailers` | Run the typed terminal event | Run once with `body = None` and `end_of_stream = true` | Dispatch `None` to companions; this is also the header-only response path |
| `TerminalBeforeTrailers` | Run the typed terminal event | Do not run | Keep the trailer lifecycle open for the actual transport trailer callback |
| Actual trailers | Already flushed at `TerminalBeforeTrailers` | Do not run | Dispatch `Some(&mut HeaderMap)` to `UpstreamResponseTrailer` companions |

A filter must tolerate `body = None` and must not treat a missing chunk as missing
completion. `Option<Duration>` delays forwarding or completion of the current task;
it does not permit sleeping inside the synchronous callback. Trailer-aware
finalization belongs to request-owned `UpstreamResponseTrailer` companions:
`Some(trailers)` is the real trailer map, while `None` confirms normal completion
without trailers. Neither lifecycle supplies a complete upstream body or permits
full-response aggregation.

Sources: [typed body driver](../../../edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs),
[read-only body trait](../../../edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs),
and [trailer companion trait](../../../edgion-gateway/src/plugins/runtime/traits/upstream_response_trailer.rs).

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

**Build timing and ownership**: the Gateway builds executable runtimes when publishing
configuration, never per request. `HttpRouteRuleUnit::compile_http_rule` and
`GrpcRouteRuleUnit::compile_grpc_rule` build rule and backend chains once before
match/hostname fan-out. Gateway API filter constructors are scope-specific:
`from_httproute_filters_for_rule()` / `from_grpcroute_filters_for_rule()` and
`from_httproute_filters_for_backend()` / `from_grpcroute_filters_for_backend()`.
There are no unscoped forwarding constructors. `new()` / `Default` construct an
empty builder, including the no-filters case.

For `EdgionPlugins`, the Controller calls schema-layer `validate_plugins()` and
reports validation errors; it does not build Gateway runtimes. The Gateway's
`compile_plugins()` builds one resource generation with `new()` and the four
`add_from_*` stage builders, collects validation errors, and seals invalid chains
fail-closed before returning `Arc<CompiledPlugins>`. `PluginStore` publishes an
immutable map through `ArcSwap`, applying the resource's invalid-update policy.

**Sharing**: `PluginRuntime` does not implement `Clone`. Compiled rules, backends,
store lookups, and request reference caches share `Arc<PluginRuntime>`, preserving
plugin state instead of rebuilding it for each reference. The removed empty-runtime
clone silently discarded the entire policy chain; it must not return. Atomic store
publication does not imply one request-wide generation across different references;
see [the reference-pinning boundary](../../04-review/plugin-runtime/intentional-design/request-plugin-reference-generation-boundary.md).

Each entry may also carry an optional `alias: String` (`^[a-zA-Z0-9._-]{1,32}$`, unique within its stage array — a duplicate is a validation error in both layers), echoed verbatim into `PluginLog.alias` so multiple entries of the same plugin type can be distinguished in the access log. The alias is also the discriminator in the entry's runtime identity `<resource key>/<alias or #index>` (`plugins::common::entry_scope`), which namespaces state a plugin keeps outside its own instance — `RateLimitRedis` Redis buckets today.

## Conditional execution

Plugins defined in EdgionPlugins are automatically wrapped with a `Conditional*` wrapper to support conditional evaluation:

| Wrapper | Wrapped object |
|---------|----------------|
| `ConditionalRequestFilter` | `RequestFilter` |
| `ConditionalUpstreamResponseFilter` | `UpstreamResponseFilter` |
| `ConditionalUpstreamResponse` | `UpstreamResponse` |
| `ConditionalUpstreamResponseBodyFilter` | `UpstreamResponseBodyFilter` |

Condition model (`PluginConditions`):
- `skip`: evaluate its `ConditionSet` first; a match skips plugin execution.
- `run`: when `skip` did not match, execute only if its `ConditionSet` matches.
- Each set has exactly one top-level `allOf` or `anyOf`. `allOf` contains
  direct conditions. `anyOf` contains direct conditions or one-level
  `{ name?, allOf: [...] }` groups. Deeper or reverse nesting, mixed top-level
  keys, empty arrays, and blank group names are invalid.

Condition kinds include:
- `KeyExist` — whether a key exists (e.g., header, query param).
- `KeyMatch` — flattened `MatchCore`: exact `values` (no singular `value`) and/or `regex`. No wildcards. `caseSensitive` defaults to true and applies to `values` only. `negate: true` is the exclude spelling.
- `TimeRange` — whether the current time falls within a given range.
- `Probability` — probabilistic execution (e.g., 10% sampling).

Every variant carries `negate` (default false), applied after an ordinary request-value miss (missing is base false; missing + `negate: true` is true). A Controller-owned reference payload that is unresolved is a separate state and always remains a non-match; `negate` must never invert it. `keyMatch` with `key.type: path` normalizes exact `values` at load time (regex is never normalized).

`keyMatch`, `jwtClaimMatch`, and `mtlsMatch` require at least one non-empty
`values` or `regex` list. This invariant is enforced by the shared `Condition`
validation path for every consumer; plugin-specific validators must not carry
partial copies of it. A present `values` list must not be empty, even when a
non-empty `regex` list is also present.

`jwtClaimMatch.claim` is required and must be non-empty after trimming. This
accessor invariant is also owned by shared `Condition` validation, before any
runtime claim extraction or `negate` handling.

`secretMatch` carries both a request-side `key: KeyGet` and a required
`secretKey` selecting exactly one entry from `secretRef`. The Controller never
ORs unrelated Secret data values. A missing, non-UTF-8, or blank selected value
leaves `resolvedValues` absent and is reported through `ResolvedRefs=False`.

Unresolved-reference policy is consumer-specific. Active entry/body `PluginConditions` seal the EdgionPlugins runtime fail-closed. `RequestRestriction` fails with the generic 500 response in `Deny` mode but treats an incomplete `Allow` group as unavailable and continues with sibling groups. `RequestAccessPolicy` likewise treats an incomplete allow candidate as unavailable. Dye remains non-blocking: an unresolved dye condition does not match. Missing or denied references retain the Controller status signal. `secretMatch` also reports an unusable selected data key; other present-but-unusable Secret consumers may rely on their resolver-specific status or warning plus Gateway enforcement. These paths do not add a per-request metric or tracing call.

The Gateway converts schema-level `ConditionSet` / `PluginConditions` values
into one shared immutable compiled representation when it builds the plugin
runtime. Besides preserving the regexes compiled during preparse, this step builds a
header-only HMAC verifier once and retains its credential map for the lifetime of the
runtime snapshot. Entry wrappers, secondary body gates, dye rules, RequestRestriction,
and RequestAccessPolicy must use this compiled representation; constructing an HMAC
verifier from schema configuration during request evaluation is a hot-path bug.

Compiled set evaluation exposes log-less and detailed synchronous paths plus a
body-aware asynchronous path. They share the same ordered short-circuit
semantics. The body phase and dye use the log-less path; request/response
filters use the detailed path and may write `cond_skip` into the outer
`PluginLog`.

**Condition match-only logging threshold.** Count atomic `Condition` leaves once
when the logical decision scope is compiled. A scope with zero or one atomic
leaf MUST NOT emit match-only condition descriptors: the plugin outcome,
`cond_skip`, or policy decision already carries the useful result, and repeating
the sole match on every request is observability noise. Emit match-only atomic
descriptors only when the scope contains more than one leaf and the matched
path helps explain a genuinely composite decision. For `PluginConditions`, the
scope is the combined `skip` plus `run` gate; for RequestRestriction it is the
whole restriction expression; for RequestAccessPolicy it is one typed
rule/candidate's supplemental condition expression. This threshold never
suppresses `cond_skip`, final allow/deny/reject records, named-group
attribution, unresolved/error diagnostics, or body-read failures. Compute the
flag at runtime construction, never by recounting leaves per request.

Body phase: call the compiled conditions' log-less evaluation path, never the
detailed path that produces a skip descriptor.

Gateway API standard filters (added through the private `add_from_httproute_filters_inner`; the public entries are the scope-explicit `from_httproute_filters_for_rule` / `from_httproute_filters_for_backend` mentioned above) are not wrapped with conditions and always execute.

Skip log format: `"skip:keyExist,hdr:X-Internal"` or `"!run:keyMatch,method"`.

### ⚠️ Critical constraint: regex must be compiled during runtime construction

`KeyMatch` supports a `regex` field. The `compiled_regex` field is annotated with `#[serde(skip)]` and is `None` after deserialization. **`compile()` MUST run before the runtime is published**; otherwise every request that hits the fallback path will execute `Regex::new()`, causing significant CPU overhead. The Controller validates the schema for status but does not build a runtime. The Gateway compiles conditions while building each EdgionPlugins resource generation.

**Validation and compile call chains**:

```
Controller EdgionPluginsHandler::preparse()
  → EdgionPlugins::validate_plugins()      # schema validation for status; no runtime

Gateway PluginStore::full_set() / partial_update()  # plugins/http/conf_handler_impl.rs
  → compile_plugins()                     # plugins/http/plugin_store.rs
      → compile_plugin_conditions()       # PluginConditions::compile() and validate()
          → Condition::compile()
              → KeyMatchCondition::compile()
                  → MatchCore::compile(normalize_paths)
      → add_from_*()                      # plugin_runtime.rs; consumes compiled conditions
      → seal_fail_closed() / seal_plugin_policy()
  → publish immutable compiled snapshot
```

**When adding a new condition kind that uses regex**: add a corresponding arm to `Condition::compile()`'s match; otherwise the new kind's regex will never be compiled.

### ⚠️ Critical constraint: no PluginLog in body phase (per-chunk path must allocate zero)

**Background**: the trait signature of `UpstreamResponseBodyFilter::run_upstream_response_body_filter` **deliberately omits** the `&mut PluginLog` parameter. The body phase covers data callbacks and terminal completion without trailers — for a long stream (gRPC server-streaming, large file downloads), this can mean hundreds to thousands of invocations per request. Any per-chunk log allocation accumulates at O(chunks × plugins) and has previously caused two bugs:

- `ExtensionRefFilter` called `start_edgion_plugins_log` in the body phase, pushing one entry per chunk into `ctx.pending_edgion_plugins_logs` without a stage drain — long streams leaked memory continuously (fixed, issue g2-001).
- `run_upstream_response_body_plugins` allocated a `PluginLog::new(filter.name())` for every filter on every chunk and immediately dropped it — pure wasted allocation, plus silent loss of `cond_skip` and other diagnostics (fixed, issue g2-004).

**Hard constraints to follow when designing new plugins, conditions, or wrappers**:

1. **Do not add `&mut PluginLog` to `UpstreamResponseBodyFilter::run_upstream_response_body_filter`**. The trait signature is the contract; it promises no per-chunk logging in this phase. If diagnostics are truly needed someday, follow #4.
2. **When implementing a new body-phase filter**, do not call `log.push*`, `log.set_*`, or any
   `tracing::` macro. Use bounded metrics or per-request aggregation emitted outside the per-chunk path.
3. **When adding a new body-phase wrapper** (similar to `ConditionalUpstreamResponseBodyFilter`), do not create temporary logs via `PluginLog::new(...)`. Condition evaluation must go through `CompiledPluginConditions::evaluate`, never `evaluate_detail`.
4. **For "body-phase summaries"** (e.g., how many times this request hit rate limiting): aggregate per-request, not push per-chunk. Add an aggregation slot in `EdgionHttpContext`, accumulate during the body phase, and emit once at the end of the request into `stage_logs`. **Never** write per-chunk execution traces into the access log.
5. **ExtensionRef body path**: use `ExtensionRefFilter::resolve_runtime_silent`, **not** `prepare_execution` (the latter calls `start_edgion_plugins_log`).
6. **Resource / configuration error diagnostics** ("referenced EdgionPlugins not found", "wrong kind", etc.) should be surfaced in Controller preparse / status conditions; **do not** rely on body-phase runtime logs as a backstop — body phase silently no-ops, and per-chunk error logs are themselves a DoS risk.

**Self-checklist** (run through this for every body-phase change during PR review):

- [ ] No `&mut PluginLog` added to the `UpstreamResponseBodyFilter` trait or its impls.
- [ ] No heap allocations on the body-phase hot path (`PluginLog::new(...)`, `String::from(...)`, `format!(...)`, `Vec::new()`, etc.).
- [ ] Condition evaluation on the body path calls `CompiledPluginConditions::evaluate` (grep for `evaluate_detail` on the body path — must be absent).
- [ ] No `ctx.pending_edgion_plugins_logs.push` or `session.start_edgion_plugins_log` calls on the body path.

## HTTP plugin inventory

The schema enum `EdgionPlugin` in
`edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs` is the authoritative inventory.
Do not rely on a hard-coded total: adding or removing an enum variant must be reflected in the
gateway creation factory and stage support. The current inventory includes Gateway API filters and
custom plugins such as FaultInjection, TraceContext, ExtProc, RequestAccessPolicy, Canary, and Wasm.
The category tables below are a conceptual guide, not an exhaustive registry.

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
| RegionRoute | `region_route` | Multi-region traffic routing. All logic AND the base `regions` table live in the plugin config: ordered routeRules (RouteByDirectRegion / RouteByKeyConfMatch / RouteByHashRange) + hashKeyGet passthrough + hashCalc writeBack + failoverTo (static redirect). An optional `overrideRef` names a `RegionRouteOverride` EdgionConfigData, looked up in EdgionConfigDataGatewayStore per request; it is data-only and never carries logic — the type declares `enable`/`active`/`regions` but only `regions` is consumed (whole-replace); the envelope `spec.enable` is what takes an overlay out of service. Fail-to-base on disabled/missing/type-mismatched/invalid overlay. No auto failover. |

### Other (8)

| Plugin | Module | Description |
|--------|--------|-------------|
| Mock | `mock` | Return a mocked response (no upstream forwarding) |
| DslPlugin | `dsl` | DSL script engine (Parser→AST→Compiler→Bytecode→VM, ~4700 lines; see [`dsl-engine.md`](dsl-engine.md) for the architecture) |
| CtxSet | `ctx_set` | Set request context variables |
| JweDecrypt | `jwe_decrypt` | Decrypt JWE-encrypted tokens |
| RequestMirrorPlugin | `request_mirror` | Request mirroring (clone the request to another backend) |
| RequestRestriction | `request_restriction` | Request restriction (by path, method, etc.) |
| AllEndpointStatus | `all_endpoint_status` | Return the status of all backend endpoints |
| DebugAccessLogToHeader | `debug_access_log` | Write access-log info to response headers (for debugging, UpstreamResponseFilter phase) |

## DSL script engine (DslPlugin)

EdgionDSL is a lightweight request/response rule engine built on a custom bytecode VM (`edgion-gateway/src/plugins/http/dsl/`, ~4700 lines). It implements both `RequestFilter` and `UpstreamResponseFilter`; the UpstreamResponseFilter path bridges the sync trait into the `async fn execute` VM via `tokio::task::block_in_place`. Panics are isolated by `vm::execute_safe` (`AssertUnwindSafe + catch_unwind`), and a layered sandbox caps source size (≤64 KB), AST nodes (≤2000), bytecode size (≤256 KB), and runtime budgets (steps / loop / call / stack / string / list / map).

The full architecture — data flow, source layout, plugin entry points, async/sync bridging, panic isolation, and `DslRuntime` lifecycle — lives in [`dsl-engine.md`](dsl-engine.md). User-facing syntax / builtin signatures / runtime tuning are documented in `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/`.

`Value::Str` / `Value::List` type design (why `Arc<str>`, not SSO or `ImmutableString`): [`skills/04-review/dsl/intentional-design/sso-migration-not-worth-it.md`](../../04-review/dsl/intentional-design/sso-migration-not-worth-it.md).

## Stream plugins

Stream plugins run at the TCP/UDP layer and use a two-stage architecture:

| Stage | Location | Timing | Plugins |
|-------|----------|--------|---------|
| Stage 1: connection context | `stream_plugin_runtime.rs`; `connection_filter_bridge.rs` for Gateway listeners | pre-TLS for TCP-based listeners; new-session admission for UDPRoute; IP/port context only | `spec.plugins` (`EdgionStreamPlugin`) |
| Stage 2: TlsRoute | `tls_route/` | post-TLS handshake, post-route match | `TlsRouteIpRestriction` |

- `StreamPlugin` trait: defines the `on_connection(ctx) -> StreamPluginResult` interface.
- `TlsRoutePlugin` trait: defines the TLS-route-level plugin interface.
- `StreamPluginRuntime`: manages the Stage-1 connection-context chain. Gateway attachments invoke it through Pingora's `ConnectionFilter`; TCPRoute invokes it per connection, and UDPRoute invokes it when admitting a new session. Gateway attachment does not cover UDP listeners because they run as independent Tokio tasks. The runtime derives `Clone` over `Vec<Arc<dyn StreamPlugin>>`, so cloning it copies the vector of `Arc` handles and shares the plugin instances.
- `TlsRoutePluginRuntime`: manages the plugin chain for the TLS-route stage. The runtime itself does not implement `Clone`, while its plugin-chain elements are `Arc<dyn TlsRoutePlugin>`.
- `StreamPluginStore`: receives EdgionPlugins resource changes via ConfHandler. It builds both runtimes once and publishes them through `Arc` fields in `CompiledStreamPlugins`.

## Directory layout

```
edgion-gateway/src/plugins/
├── http/                          # HTTP plugins; authoritative variants live in EdgionPlugin
│   ├── basic_auth/                # one subdirectory per plugin
│   ├── jwt_auth/
│   ├── cors/
│   ├── dsl/                       # DSL engine (with lang/ subdirectory: AST, compiler, VM)
│   ├── ...
│   ├── common/                    # shared plugin utilities (auth_common, http_client)
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
        ├── extension_ref.rs       # ExtensionRef (route/backend filter list referencing EdgionPlugins, Gateway API standard) — nested form removed; see docs/en/migrations/<version>-extension-ref.md
        ├── cors_filter.rs
        └── debug_access_log.rs
```
