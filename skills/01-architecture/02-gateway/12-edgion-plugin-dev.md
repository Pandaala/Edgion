# HTTP Plugin Development

Use this for tasks such as:

- Adding or extending an HTTP-layer `EdgionPlugins` plugin.
- Adjusting `PluginRuntime`, `ExtensionRef` (route/backend filter list reference), `PluginSession`, conditional execution, or PluginLog.
- Troubleshooting "the plugin is configured but the route does not execute it / the execution order is wrong / the Secret is not resolved / the access log is wrong".

## Read these real entry points first

- [../../../edgion-resources/src/resources/edgion_plugins/mod.rs](../../../edgion-resources/src/resources/edgion_plugins/mod.rs)
- [../../../edgion-resources/src/resources/edgion_plugins/entry.rs](../../../edgion-resources/src/resources/edgion_plugins/entry.rs)
- [../../../edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs](../../../edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs)
- [../../../edgion-gateway/src/plugins/runtime/plugin_runtime.rs](../../../edgion-gateway/src/plugins/runtime/plugin_runtime.rs)
- [../../../edgion-gateway/src/plugins/runtime/conditional_filter.rs](../../../edgion-gateway/src/plugins/runtime/conditional_filter.rs)
- [../../../edgion-gateway/src/plugins/runtime/traits/session.rs](../../../edgion-gateway/src/plugins/runtime/traits/session.rs)
- [../../../edgion-gateway/src/plugins/runtime/log.rs](../../../edgion-gateway/src/plugins/runtime/log.rs)
- [../../../edgion-gateway/src/plugins/http/mod.rs](../../../edgion-gateway/src/plugins/http/mod.rs)
- [../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs](../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs)
- [../../../edgion-gateway/src/plugins/http/conf_handler_impl.rs](../../../edgion-gateway/src/plugins/http/conf_handler_impl.rs)
- [../../../edgion-gateway/src/plugins/http/plugin_store.rs](../../../edgion-gateway/src/plugins/http/plugin_store.rs)
- [../../../edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs](../../../edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs)

## Key mental model

HTTP plugins now have two distinct runtime build paths. Identify which one you are touching first:

### 1. The `EdgionPlugins` resource path

The chain is:

1. The Controller's `EdgionPluginsHandler.preparse()` calls `ep.preparse()`.
2. `EdgionPlugins::preparse()` builds `spec.plugin_runtime` and collects `preparse_errors`.
3. The Gateway's `PluginStore` also calls `preparse()` again on `full_set` / `partial_update`.
4. At runtime, the resource is fetched through `PluginStore` and its own `plugin_runtime` is executed.

This path mainly serves:

- Reusable plugin resources such as `EdgionPlugins`.
- HTTP plugins that depend on Secret resolution.

### 2. The Route / Gateway API filter path

The chain is:

- During `HTTPRoute` / `GRPCRoute` preparse, the runtime is built per scope: rule-level uses `PluginRuntime::from_httproute_filters_for_rule()` / `from_grpcroute_filters_for_rule()`; backend-level uses `from_httproute_filters_for_backend()` / `from_grpcroute_filters_for_backend()`. The legacy `from_httproute_filters()` / `from_grpcroute_filters()` are `#[deprecated]` and forward to the new entry points with rule scope by default.

This path mainly serves:

- `RequestHeaderModifier`
- `ResponseHeaderModifier`
- `RequestRedirect`
- `URLRewrite`
- `RequestMirror`
- The `ExtensionRef` wrapper itself.

The most common pitfall is mixing the two paths:

- Modifying the `EdgionPlugin` enum without wiring `PluginRuntime::create_*_from_edgion()` — the `EdgionPlugins` resource will not execute it.
- Looking only at route filter preparse and ignoring `PluginStore` and `ExtensionRef`.
- Changing only the Gateway side without letting the Controller's `preparse()` / `update_status()` surface validation issues.

## Plugin type decision

| Scenario | Choose `EdgionPlugin` | Choose Gateway API filter |
|----------|------------------------|----------------------------|
| Need Header / Cookie / Query / Path access | Yes | No |
| HTTP semantics: auth, rate limit, rewrite, mirror | Yes | No |
| Upstream response headers / response body chunks | Yes | No |
| Cross-plugin `PluginSession` ctx variables | Yes | No |
| Standard Gateway API filters (HeaderModifier, Redirect, URLRewrite, Mirror) | No | Yes — under `gateway_api_filters/` |
| Only depends on IP / listener port / SNI / mTLS | No | No, switch to a Stream plugin |

> Related: [13-stream-plugin-dev.md](13-stream-plugin-dev.md)

## Choose the plugin phase

| Phase | trait | When it runs | Common uses |
|-------|-------|--------------|-------------|
| Request phase | `RequestFilter` | Before forwarding to upstream | Auth, RateLimit, Rewrite, Mirror, direct reject |
| Response header phase | `UpstreamResponseFilter` | When upstream response headers arrive | Modify response headers, fast decision based on header |
| Response body phase | `UpstreamResponseBodyFilter` | Each body chunk | Bandwidth limit, chunk inspection (**the trait signature deliberately omits `&mut PluginLog`**; the per-chunk path must allocate zero — see [05-plugin-system.md](05-plugin-system.md#-critical-constraint-no-pluginlog-in-body-phase-per-chunk-path-must-allocate-zero)) |
| Response-end phase | `UpstreamResponse` | After the full response ends | Logic that needs the complete response context (rare) |

Default to `RequestFilter`.
If you don't need upstream response info, don't push the logic into a later phase.

## Minimum change checklist

### A regular HTTP plugin

You typically touch at least these locations:

1. `edgion-resources/src/resources/edgion_plugins/plugin_configs/<your_plugin>.rs`
2. `edgion-resources/src/resources/edgion_plugins/plugin_configs/mod.rs`
3. `edgion-resources/src/resources/edgion_plugins/mod.rs`
4. `edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs`
5. `edgion-gateway/src/plugins/http/<your_plugin>/mod.rs`
6. `edgion-gateway/src/plugins/http/<your_plugin>/plugin.rs`
7. `edgion-gateway/src/plugins/http/mod.rs`
8. `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`

### If the plugin depends on a Secret

Continue with:

- [../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs](../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs)

This handler currently covers:

- `BasicAuth`
- `JwtAuth`
- `JweDecrypt`
- `HeaderCertAuth`
- `HmacAuth`
- `KeyAuth`
- `OpenidConnect`

If your new plugin also depends on a Secret, add Secret resolution and `secret_ref_manager` reference registration.

## Recommended development order

### 1. Decide the plugin type first (see the decision table above)

### 2. Define the configuration struct

Typically located in:

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/`

Notes for the configuration struct:

- `#[serde(rename_all = "camelCase")]`
- Runtime-derived fields use `#[serde(skip)]` or `#[schemars(skip)]`.
- If the configuration may be structurally invalid, implement `get_validation_error()`.

If you can reuse an existing configuration, do not copy a near-duplicate schema.

### 3. Wire it into the `EdgionPlugin` enum

Modify:

- [../../../edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs](../../../edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs)

At minimum, add:

- The enum variant.
- `type_name()`.
- Related re-exports.

### 4. Implement the plugin

Typically located in:

- `edgion-gateway/src/plugins/http/<your_plugin>/mod.rs`
- `edgion-gateway/src/plugins/http/<your_plugin>/plugin.rs`

Good references in the current repo:

- Simplest request plugin: `mock`.
- Plugins that depend on ctx variables: `real_ip`, `ctx_set`.
- Plugins that depend on Secrets and complex validation: `jwt_auth`, `openid_connect`, `key_auth`.
- Body filter: `bandwidth_limit`.

### 5. Register the construction logic in `PluginRuntime`

Must read:

- [../../../edgion-gateway/src/plugins/runtime/plugin_runtime.rs](../../../edgion-gateway/src/plugins/runtime/plugin_runtime.rs)

The current real entry point is this file, not the old `runtime.rs` referenced by stale docs.

Functions you typically need to change:

- `create_request_filter_from_edgion()`
- `create_upstream_response_filter_from_edgion()`
- `create_upstream_response_from_edgion()`
- `create_upstream_response_body_filter_from_edgion()`
- `get_plugin_validation_error()`
- `get_plugin_name()`

If you only modify the enum without modifying these constructors, the Gateway will not execute your plugin.

### 6. If the plugin depends on Secrets, wire the Controller resolution chain

Currently, Secret resolution is not silently done inside the runtime; it lives in:

- `EdgionPluginsHandler.parse()`

This is responsible for:

- Resolving Secrets.
- Writing the result into `resolved_*` runtime fields.
- Registering cascading references with `secret_ref_manager`.
- Cleaning up references via `on_delete()` on deletion.

If your plugin depends on Secrets but is not wired into this chain, you usually see:

- The YAML applies fine.
- Status may even pass.
- But the runtime never gets the actual credentials.

### 7. Finally, add user docs and tests

If the plugin is a user-facing capability, in addition to skill / dev-guide, also consider user-guide:

- `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/`
- `docs/en/user-guide/http-route/filters/edgion-plugins/`

## Conditional execution and `ctx` variables

`RequestFilterEntry` / `UpstreamResponseFilterEntry` / other entries already support:

- `enable`
- `conditions`

The actual conditional-execution wrapper is in:

- [../../../edgion-gateway/src/plugins/runtime/conditional_filter.rs](../../../edgion-gateway/src/plugins/runtime/conditional_filter.rs)

Important distinction:

- Entries inside `EdgionPlugins` are automatically wrapped with a `Conditional*` wrapper.
- Standard Gateway API filters added through the scope-explicit entries (`from_httproute_filters_for_rule` / `from_httproute_filters_for_backend` and the gRPC variants) currently are NOT auto-wrapped with this conditional wrapper (they are added through the private `add_from_httproute_filters_inner`).

For inter-plugin value passing, prefer:

- `session.set_ctx_var()`
- `session.get_ctx_var()`
- `session.remove_ctx_var()`
- `key_get()` / `key_set()`

Don't reparse the entire request just to read a previous plugin's result.

## ExtensionRef has only one form: HTTPRoute / GRPCRoute filter list

> **Note:** `EdgionPlugin::ExtensionRef` (the nested form, i.e. writing `type: ExtensionRef` inside `EdgionPlugins.spec.*Plugins`) has been removed (release `<version>`) — "plugin referencing plugin" is no longer supported. Route- and backend-level `ExtensionRef` (HTTPRoute / GRPCRoute / HTTPBackendRef filter lists, the Gateway API standard) is retained. See `docs/zh-CN/migrations/<version>-extension-ref.md` for details.

```yaml
HTTPRoute.rules[].filters:
  - type: ExtensionRef
    extensionRef: { kind: EdgionPlugins, name: B }
```

- Single declaration → the three stages auto fan-out (request / upstream_response_filter / upstream_response_body_filter).
- Each stage automatically pulls the same-stage subset from B.
- This is the Gateway API standard convention, equivalent to "inserting B as a bundle into this route".
- During build, the entries are cloned into three Vecs; see the `HTTPRouteFilterType::ExtensionRef` branch in `plugin_runtime.rs::add_from_httproute_filters_inner`.
- Per-request snapshot optimization is already in place (the `ctx.resolved_route_ext_refs` index path).

### Two recommended patterns for multi-plugin reuse

- **Aggregate pattern**: combine multiple plugins into one `EdgionPlugins` resource; the route only needs one `ExtensionRef`.
- **Side-by-side pattern**: list multiple `ExtensionRef`s in `HTTPRoute.rules[].filters`, each pointing to a different `EdgionPlugins`.

```yaml
HTTPRoute.rules[].filters:
  - type: ExtensionRef
    extensionRef: { kind: EdgionPlugins, name: shared-bundle-1 }
  - type: ExtensionRef
    extensionRef: { kind: EdgionPlugins, name: shared-bundle-2 }
```

Reuse is achieved by "multiple Routes referencing the same resource" (analogous to Envoy ECDS), not by "plugin referencing plugin".

### Auditing nested usage in the current cluster (forensic tool)

The Controller admin API still keeps a diagnostic endpoint to enumerate residual nested ExtensionRef usage in stored YAML (forensic only; the CRD schema no longer accepts the nested form):

```
GET /api/v1/diagnostics/nested_ext_ref_usage
```

Implementation: `edgion-controller/src/api/diagnostics_handlers.rs`.

## `ExtensionRef` execution chain

If the plugin is referenced by a route through `ExtensionRef`, focus on:

- [../../../edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs](../../../edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs)

Current behavior:

- At runtime, fetch the `EdgionPlugins` from `PluginStore` by `namespace/name`.
- Execute using `spec.plugin_runtime`.
- Push plugin logs into the dedicated `edgion_plugins` log structure.

If you are debugging "ExtensionRef does not take effect / plugin logs are missing", start tracing from here.

## Reading the request path

> Authoritative architecture doc: [uri-normalization/SKILL.md](uri-normalization/SKILL.md)

When path normalization is enabled, the session trait exposes 5 path/uri read methods. Choosing the wrong view leads to ACL bypass or audit info loss.

> `get_query()` returns the **raw, current** query string verbatim — reflects any in-request URI rewrite (`set_upstream_uri`).
> `get_query_param(name)` returns the **decoded, entry-frozen** value of a single key — sourced from the request as the client sent it, immune to in-request URI rewrites, and consistent with what HTTPRoute `queryParams` matchers see.

| Method | View | Return type | When to use |
|--------|------|-------------|-------------|
| `session.get_path()` | current (normalized) | `&str` | ACL decisions, route matching (**preferred**, no extra alloc) |
| `session.get_request_path()` | entry (frozen) | `&str` | Auditing, security investigation, scenarios needing the client's original path |
| `session.get_uri()` | current (normalized) | `String` | Full URI of path + query (1 alloc) |
| `session.get_request_uri()` | entry (frozen) | `String` | Full URI for auditing (entry raw, with query) |
| `session.get_query()` | current | `Option<String>` | Get the current query (not normalized) |
| `session.get_query_param(name)` | entry (frozen) | `Option<String>` | Get one query parameter (**percent-decoded**, `+` → space). Backed by per-request `EdgionHttpContext::cached_query`, sourced from `entry_raw_query` — shared with route matcher; safe to call from many plugins; immune to mid-request URI rewrites. |

**Default to `get_path()`**. Only use `get_request_path()` / `get_request_uri()` when you explicitly need the client's original form.

## Modifying the request path (Rewrite plugins)

**The only correct approach**: use the trait method `session.set_upstream_uri(&new_uri)`.

```rust
// Correct: rewrite path through the trait method
let new_uri = format!("/v2{}", session.get_path());
session.set_upstream_uri(&new_uri)?;

// Forbidden: directly touching pingora raw fields
// session.req_header_mut().set_uri(parsed_uri);  // Not allowed!
```

**Why directly calling `req_header_mut().set_uri()` is forbidden**:

- The ctx cache (`normalized_current_path`) is not refreshed automatically.
- Subsequent `session.get_path()` returns **stale** (the old path).
- The access_log `$uri` records the old path.
- Route decisions and ACL plugins read the wrong path.
- **(v4)** The `ctx.uri_finalized` flag isn't set, causing the `pg_upstream_peer::upstream_peer_http` single-point decision to potentially write again and overwrite the rewrite result.

The internal implementation of `session.set_upstream_uri()` (`session_adapter::set_upstream_uri`) automatically calls `ctx.refresh_normalized_path()` **and sets `ctx.uri_finalized = true`**, ensuring cache consistency and that the single-point decision skips rewriting. All rewrite plugins (`proxy_rewrite`, `url_rewrite`, `request_redirect`, DSL `req.set_uri`, etc.) already use this path; **the plugin layer needs no changes**.

**(v4) The only 2 legitimate `req_header_mut().set_uri()` write sites in the entire codebase** (lint Rule 2):

1. `session_adapter::set_upstream_uri` (the wrapper, the rewrite-plugin entry).
2. The `pg_upstream_peer::upstream_peer_http` single-point decision block (spec §14, writes the normalized final state to upstream).

Both must immediately be followed by `ctx.uri_finalized = true`. Any third site is a violation — flag it during review; see `skills/04-review/uri-normalization/SKILL.md` Rule 2.

**(v4) View decision for forwarding path to upstream** (relevant if your plugin influences upstream path selection):

`upstream_peer_http` makes a single-point decision at entry whether the upstream receives the raw or normalized path; 5 channels by priority:

1. `ctx.uri_finalized` already true (rewrite plugin already wrote) → no-op.
2. HTTPRoute annotation `edgion.io/forward-raw-path: "true"|"false"` → forced.
3. `external_jump.preserve_raw_path = true` (region_route-class gateway hop) → raw.
4. Global `forward_normalized_path = false` → raw.
5. Default → write normalized.

**Plugin interaction notes**:
- Your rewrite-class plugin's output (`set_upstream_uri`) has the **highest priority** — the single-point decision sees `uri_finalized = true` and skips.
- Plugins that set `external_jump` (such as region_route / dynamic_external_upstream) need to choose `preserve_raw_path` explicitly: `true` for gateway-to-gateway hops, `false` for ordinary external servers.
- See spec `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §14 for details.

## `PluginSession` and common APIs

The most-used read/write entry points are at:

- [../../../edgion-gateway/src/plugins/runtime/traits/session.rs](../../../edgion-gateway/src/plugins/runtime/traits/session.rs)

High-frequency APIs:

- `header_value()` / `request_headers()`
- `get_path()` / `get_request_path()` / `get_uri()` / `get_request_uri()` / `get_query()`
- `client_addr()` / `remote_addr()` / `client_cert_info()`
- `set_request_header()` / `append_request_header()` / `remove_request_header()`
- `set_upstream_uri()` / `set_upstream_host()` / `set_upstream_method()`
- `write_response_header()` / `write_response_body()` / `shutdown()`
- `set_ctx_var()` / `get_ctx_var()`
- `key_get()` / `key_get_local()` / `key_set()`

If the plugin will call external HTTP services, prefer reusing:

- `edgion-gateway/src/plugins/http/common/http_client.rs`
- `edgion-gateway/src/plugins/http/common/auth_common.rs`

## `PluginLog` rules

Entry point:

- [../../../edgion-gateway/src/plugins/runtime/log.rs](../../../edgion-gateway/src/plugins/runtime/log.rs)

Keep the rules simple:

- Record results, not lengthy internal traces.
- Use short phrases ending with semicolons, e.g., `OK u=jack; `.
- For verbose debugging, use `tracing::debug!()`; do not treat the access log as a debug dump.
- Conditional skip (`cond_skip`) is written automatically by the conditional wrapper — do not invent your own format.
- **No PluginLog in the `UpstreamResponseBodyFilter` phase**: the trait signature deliberately omits the log parameter; the body phase fires once per chunk, and per-chunk logging blows up. See the full constraint in [05-plugin-system.md](05-plugin-system.md#-critical-constraint-no-pluginlog-in-body-phase-per-chunk-path-must-allocate-zero).

## `ErrTerminateRequest` — write the response first

When a plugin returns `ErrTerminateRequest`, it **SHOULD** write a complete
response (status + headers + body) to the session before returning:

```rust
// Correct: write response, then return ErrTerminateRequest
session.write_response_header(...).await?;
session.write_response_body(Some(Bytes::from("Forbidden")), true).await?;
return Ok(PluginRunningResult::ErrTerminateRequest);

// Wrong: return ErrTerminateRequest without writing — triggers runtime fallback
return Ok(PluginRunningResult::ErrTerminateRequest);
```

If the plugin cannot write a response at its current stage (e.g. a sync hook
where `await` is unavailable), use `ErrResponse { status, body }` instead —
the runtime writes the response on the plugin's behalf.

**Runtime fail-safe**: a plugin that violates this convention receives a 502
Bad Gateway fallback from the runtime, and `PluginTerminateWithoutResponse`
appears in the access log `errors[]` field. Fix the plugin so it writes its
own response and the marker disappears.

## Project-specific rules

### No Consumer model

Do not design with the Kong / APISIX "Consumer -> Plugin" mindset.

In this repo:

- Credentials live mainly in Kubernetes Secrets.
- Plugins reference Secrets directly or use post-resolution runtime fields.
- Upstream identity is usually conveyed via request headers or ctx variables.

### Comments and logs in code use English

This is a general repo-wide convention; stay consistent with existing plugins.

## Validation

All plugin configs must `impl PluginValidation` (trait defined in `edgion-resources/src/resources/edgion_plugins/plugin_validation.rs`):

```rust
use crate::types::resources::edgion_plugins::PluginValidation;

impl PluginValidation for MyPluginConfig {
    fn validation_error(&self) -> Option<String> {
        // stateless field-level checks — no mutation, no I/O
        if self.required_field.is_empty() {
            return Some("requiredField is required".to_string());
        }
        None
    }
}
```

**Requirements:**
- Implementation must be stateless (`&self`, no side effects) — safe to call during controller preparse
- **Do not** introduce `validation_error: Option<String>` cached fields — this legacy pattern causes preparse path to miss errors (see t1-002 root cause)
- If config needs runtime compilation (regex / template / DashMap), keep that logic in `compile()` / constructors returning `Result<(), String>` — **separate field validation from compilation**
- Add the new arm to `EdgionPlugin::validation_error()` match in `edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs` — the exhaustive match enforces this at compile time (missing arm = compile error)

**Required regression test (add to every new plugin):**
```rust
#[test]
fn validation_error_returns_field_error_without_validate_call() {
    let config = MyPluginConfig { /* invalid field */ ..Default::default() };
    assert!(config.validation_error().is_some());
    // If your config has a cached field, assert it's not touched:
    // assert!(config.validation_error_cached.is_none());
}
```

## Testing recommendations

Cover these four layers in priority order:

1. Unit tests for configuration and `preparse()`.
2. Plugin unit tests driven by `MockPluginSession`.
3. If Secret resolution is involved, add unit tests for `EdgionPluginsHandler`.
4. Integration tests via `ExtensionRef` or actual route hits.

`PluginSession` already has `automock`, so unit tests can use `MockPluginSession` directly.

### If the plugin supports hot reload (`partial_update` / `full_set`)

Verifying initial state is not enough — add a dynamic scenario that exercises the "YAML update → runtime behavior change" path. Templates: remove a credential (BasicAuth) or switch an enum field (RateLimit `onMissingKey`).

- Look at existing scenarios to get started: `examples/code/client/dynamic/scenarios/basic_auth_dynamic.rs` / `rate_limit_dynamic.rs`.
- conf directory convention: `examples/test/conf/EdgionPlugins/<Plugin>/DynamicTest/{initial,updates}/` (`load_conf.sh` automatically skips `updates/` and `delete/`).
- For the detailed steps, CLI entry, and registration sites: [../../05-testing/01-integration-testing.md "Dynamic (hot reload) testing" section](../../05-testing/01-integration-testing.md).

> **Pitfalls**: state-cached fields such as RateLimit sliding-window counters can false-pass on hot reload (the old cache hits). Prefer verifying **logic-branch** changes such as "the entire ref is removed" or "an enum field is toggled" — do not just verify "password content modified" or "a rate value changed". (BasicAuth no longer keeps a result cache after bcrypt was dropped, so the structural-reload pattern is now mostly a regression-safety template for it rather than a cache-bypass workaround.)

Other references:

- `edgion-resources/src/resources/edgion_plugins/tests.rs`
- Each plugin's own `plugin.rs` / `tests.rs`
- [../../05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md)

## Review checklist

- Was the plugin phase chosen correctly?
- Are the `EdgionPlugin` enum, runtime constructors, and module exports all wired up?
- Was the obsolete `runtime.rs` doc path used by mistake instead of the current `plugin_runtime.rs`?
- If a Secret is required, are `EdgionPluginsHandler.parse()` and `on_delete()` wired in?
- Is `PluginSession` / `ctx` used correctly instead of bypassing the abstraction?
- Is `PluginLog` concise — does it avoid stuffing large debug info into the access log?
- Are success, failure, skip/run conditions, and combined execution all covered by tests?

## Related

- [docs/zh-CN/dev-guide/http-plugin-development.md](../../../docs/zh-CN/dev-guide/http-plugin-development.md)
- [observability/00-access-log.md](../../03-coding/observability/00-access-log.md)
- [13-stream-plugin-dev.md](13-stream-plugin-dev.md)
- [14-link-sys-dev.md](14-link-sys-dev.md)
- [../../05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md)
