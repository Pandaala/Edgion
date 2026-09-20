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

1. The Controller's handler calls schema-layer `EdgionPlugins::validate_plugins()` and reports validation errors through status. It does not construct an executable `PluginRuntime`.
2. On Gateway `full_set` / `partial_update`, the configuration handler calls `compile_plugins()` to build conditions and all four stage chains, collect errors, and seal invalid chains fail-closed.
3. The resulting `Arc<CompiledPlugins>` owns the resource and `Arc<PluginRuntime>`. `PluginStore` publishes an immutable snapshot, applying the resource's invalid-update policy.
4. Runtime references share the stored `Arc<PluginRuntime>`. The runtime itself is not `Clone`; rebuilding or empty-cloning it would lose the shared policy chain or its state.

This path mainly serves:

- Reusable plugin resources such as `EdgionPlugins`.
- HTTP plugins that depend on Secret resolution.

### 2. The Route / Gateway API filter path

The chain is:

- During Gateway `compile_http_rule()` / `compile_grpc_rule()`, the runtime is built once per rule and backend before match/hostname fan-out: rule-level uses `PluginRuntime::from_httproute_filters_for_rule()` / `from_grpcroute_filters_for_rule()`; backend-level uses `from_httproute_filters_for_backend()` / `from_grpcroute_filters_for_backend()`. These are the only Gateway API filter constructors; unscoped forwarding constructors have been removed. Share the resulting runtime Arcs across compiled match units.

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
| Request phase | `RequestFilter` | Before forwarding to upstream | Auth, RateLimitLocal, Rewrite, Mirror, direct reject |
| Response header phase | `UpstreamResponseFilter` | When upstream response headers arrive | Modify response headers, fast decision based on header |
| Async final response header phase | `UpstreamResponse` | After synchronous header filtering, retry arbitration, and normalization; before downstream header commit and body streaming | Await header decisions, modify headers, install bounded streaming companions |
| Response body phase | `UpstreamResponseBodyFilter` | Data callbacks and terminal completion without trailers | Bandwidth limit, chunk inspection (**the trait signature deliberately omits `&mut PluginLog`**; the per-chunk path must allocate zero — see [05-plugin-system.md](05-plugin-system.md#-critical-constraint-no-pluginlog-in-body-phase-per-chunk-path-must-allocate-zero)) |

Both response-header chains skip interim 1xx (for example 100/103); 101 is a
terminal upgrade and is not skipped as interim. `UpstreamResponse` runs from
Pingora's `response_filter` callback on the final response candidate. It has no
complete-response or complete-response-body view. Keep its awaited header decision
separate from later body filters, bounded streaming processors, and trailer
companions; the main upstream response always streams. The detailed ordering and
source links are in [the execution model](05-plugin-system.md#four-phase-execution-model).

`UpstreamResponseBodyFilter` also receives no-trailers terminal completion,
including header-only responses with `body = None` and `end_of_stream = true`.
It does not run for `TerminalBeforeTrailers`; do not treat that boundary as EOS.
Use `UpstreamResponseTrailer` for trailer-aware finalization rather than inferring
trailer presence from body callbacks. See [body and trailer completion](05-plugin-system.md#response-body-and-trailer-completion)
for the event mapping and driver/trait sources.

Default to `RequestFilter`.
If you don't need upstream response info, don't push the logic into a later phase.

For response-body callbacks, the driver runs writable streaming processors first
and then ordinary `UpstreamResponseBodyFilter` plugins on each `Data` event. A
`Data` event with `end_of_stream = true` closes a response without trailers and
then dispatches `None` to the trailer companions. Header-only and other synthetic
no-trailers completion arrives as `TerminalWithoutTrailers`: streaming processors
receive that typed event, ordinary body plugins run once with `body = None` and
`end_of_stream = true`, and the driver dispatches `None` to the trailer companions.

`TerminalBeforeTrailers` is different. Only the streaming-processor chain receives
that typed event; ordinary `UpstreamResponseBodyFilter` plugins do not run. The
subsequent transport trailer callback passes the actual mutable trailer map to
request-owned `UpstreamResponseTrailer` companions. Keep body processing bounded
to the current chunk or semantic window; none of these callbacks may aggregate the
main upstream response.

## Minimum change checklist

### A regular HTTP plugin

You typically touch at least these locations:

1. `edgion-resources/src/resources/edgion_plugins/plugin_configs/<your_plugin>.rs`
2. `edgion-resources/src/resources/edgion_plugins/plugin_configs/mod.rs`
3. `edgion-resources/src/resources/edgion_plugins/mod.rs`
4. `edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs` — the enum variant plus
   **six** exhaustive matches: `type_name`, `validation_error`, `body_requirement`,
   `dye_capabilities`, `dye_type_prefix`, `key_header_names`. All six are exhaustive, so the
   compiler tells you if you miss one.
5. `edgion-gateway/src/plugins/http/<your_plugin>/mod.rs`
6. `edgion-gateway/src/plugins/http/<your_plugin>/plugin.rs`
7. `edgion-gateway/src/plugins/http/mod.rs`
8. `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` — `get_plugin_name` plus all four
   `create_*_from_edgion` constructors (the three where the plugin is invalid take a
   `CreateResult::WrongStage` arm).

Two more that the compiler does **not** catch:

9. `config/crd/edgion-crd/edgion_plugins_crd.yaml` — add the type to the `type:` enum of every
   stage the plugin is valid at. Miss this and the CRD prunes/rejects the plugin at admission,
   i.e. it is un-appliable. Guarded by `edgion-resources/tests/plugin_crd_type_coverage.rs`.
10. `edgion-resources/src/resources/edgion_plugins/validate.rs` — **only if the plugin is not
    request-stage-only.** `HTTP_PLUGIN_STAGE_EXCEPTIONS` is the single source of truth for stage
    applicability (request-only is the default and needs no entry); the CRD guard reads it via
    `applicable_stages_by_name`. A key that names no real variant fails
    `stage_exception_keys_are_real_variants`.

Integration-test registration (suite module, `test_client.rs`, `run_integration.sh`,
`capabilities.tsv`) is covered in
[../../05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md). Note
that a suite added to the `-r <Resource>` sweep but not the default sweep is **never run by CI** —
`every_targeted_sweep_suite_also_runs_in_the_default_sweep` in `test_client.rs` guards that.

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
- External-runtime plugin (embeds a separate execution engine — global engine/cache/pool + host ABI bridge, guest failure isolated from the gateway process): `wasm` (`edgion-gateway/src/plugins/http/wasm/`).

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

### 6b. If the plugin uses EdgionConfigData refs (`allowRefs`/`denyRefs`, `overrideRef`, or `activeProfileRef`)

Cross-namespace EdgionConfigData refs require controller-side handling for the
`ReferenceGrant` / `visibility` gate:

1. **Register refs** in `EdgionPluginsHandler.parse()`: call the shared ref-registration helper
   (see `register_ip_allow_deny_refs` and the `overrideRef` / `activeProfileRef` patterns) to
   record the reverse dependency in `EdgionConfigDataRefManager`. This ensures the EdgionPlugins
   resource is requeued when the referenced EdgionConfigData changes (including when a grant is
   added or removed).
2. **Mark `ref_denied`**: after registration, call the `set_ref_denied` helper (checks
   `is_reference_grant_validation_enabled()`, target `visibility`, and the `ReferenceGrantStore`)
   to stamp `ref_denied` on any cross-namespace ref that is not permitted.
3. **Check `ref_denied` in `update_status`**: map `ref_denied` → `ResolvedRefsError::RefNotPermitted`
   (reason `RefNotPermitted`) before emitting the `ResolvedRefs` condition.
4. **Gateway data-plane**: when consuming the ref, check `ref_denied`; if set, apply the correct
   fail behavior: `allowRef` → deny-all fail-closed; `denyRef` → under-block (omit that deny set);
   `overrideRef` / `activeProfileRef` → fall to base.
5. **Plugins that address a route backendRef by name** (a jump publisher such as
   `DynamicInternalUpstream` or `Canary`) must resolve through
   `RouteRules::find_backend_by_name` against the route's *compiled* backend view. Never scan
   `route_unit.rule.backend_refs` yourself: the schema view carries no `ref_denied`, so a hand-
   rolled scan silently accepts a denied target. See
   [`skills/04-review/routing/regression-guards/denied-named-jump-fails-closed.md`](../../04-review/routing/regression-guards/denied-named-jump-fails-closed.md).

**Cross-namespace `allowRefs`/`denyRefs` — key semantics for operators:**
A denied `allowRef` is deny-all fail-closed (whitelist intent preserved); a denied `denyRef` is
under-block (the deny set is missing). Operators who want cross-ns IpList refs must either set
`spec.visibility: Cluster` on the target or author a `ReferenceGrant` in the target namespace.
See [15-edgion-config-data.md — Cross-Namespace Reference Grants](../../01-architecture/05-resources/15-edgion-config-data.md#cross-namespace-reference-grants).

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

> **Note:** `EdgionPlugin::ExtensionRef` (the nested form, i.e. writing `type: ExtensionRef` inside `EdgionPlugins.spec.*Plugins`) has been removed (release `<version>`) — "plugin referencing plugin" is no longer supported. Route- and backend-level `ExtensionRef` (HTTPRoute / GRPCRoute / HTTPBackendRef filter lists, the Gateway API standard) is retained. See `docs/en/migrations/<version>-extension-ref.md`; the paired Chinese page is under `docs/zh-CN/migrations/`.

```yaml
HTTPRoute.rules[].filters:
  - type: ExtensionRef
    extensionRef: { kind: EdgionPlugins, name: B }
```

- Single declaration → all four stages auto fan out (`Request`, synchronous
  `UpstreamResponseFilter`, per-chunk `UpstreamResponseBodyFilter`, and async
  `UpstreamResponse`).
- Each stage automatically pulls the same-stage subset from B.
- This is the Gateway API standard convention, equivalent to "inserting B as a bundle into this route".
- During build, the stage-specific `ExtensionRefFilter` clones are inserted into
  the four runtime vectors; see the `HTTPRouteFilterType::ExtensionRef` branch in
  `PluginRuntime::add_from_httproute_filters_inner` (and the corresponding gRPC
  branch).
- A request captures one immutable `GatewayScope`, which retains the Gateway
  identity and `http_keys`; its `.scope` is the class-scoped `PluginScope` used
  for ExtensionRef resolution. Rule references resolve positionally from that
  `PluginScope` into `ctx.resolved_route_ext_refs`. Backend references resolve
  lazily from the same `PluginScope` when the selected backend first uses them,
  then pin either the hit or the miss across retries. A captured owner whose
  `gateway_key` differs from the matched Gateway is rejected rather than mixed
  with that Gateway's policy.

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

- `ExtensionRefFilter` never looks up the live `PluginStore` directly. Rule
  filters read their position from `ctx.resolved_route_ext_refs`; backend filters
  use the request's pinned backend-reference cache. Both projections come from
  the `.scope` `PluginScope` inside the same request-captured `GatewayScope`.
- Execute using the `Arc<PluginRuntime>` stored in the matching immutable
  `CompiledPlugins` entry.
- Push plugin logs into the dedicated `edgion_plugins` log structure.

If you are debugging "ExtensionRef does not take effect / plugin logs are missing", start tracing from here.

## Optional read-only request-body observation

The Gateway has one internal, request-scoped observer seam for protocol
frontends that need bounded metadata from the proxied request body without
requiring full-body capture. It is not a general operator plugin API.

- Registration is available only from the request stage through
  `PluginSession::install_request_body_observer`. A false return always disables
  observation and is failure-open; it must never reject the request.
- Observers run synchronously after existing mutable request-body stream
  handlers and before the mirror tee/upstream forwarding. They receive borrowed
  immutable bytes and cannot replace, retain, replay, or aggregate the body.
- The host caps slots, bytes, events, declared work, and retained scratch.
  Retry reuses the canonical Pingora replay path and resets only fixed semantic
  state; request-wide work counters do not reset.
- BodyStorage remains the only request-body cache/spill owner. Installing an
  observer does not enable capture, change framing, or disable retry.
- Chunk processing emits no logs, metrics, or tracing. Exactly one static,
  payload-free outcome per observer is drained through the existing access-log
  stage at EOS or request cleanup.
- Observer-owned factory, callback, retained-size, finalize, and destructor
  panics use the canonical redacted panic scope and cannot expose their payload.

Protocol parsing belongs to the registering frontend. It must use fixed-capacity
state and charge semantic work before scanning; the generic seam does not
contain JSON, protocol-method, or content-type recognition.

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

Both must immediately be followed by `ctx.uri_finalized = true`. Any third site is a violation — flag it during review; see `skills/04-review/uri-normalization/mandatory-design/normalization-contract.md` Rule 2.

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
- `direct_peer_ip()` / `remote_ip()` / `client_cert_info()`
- `set_request_header()` / `append_request_header()` / `remove_request_header()`
- `set_request_header_value()` preserves typed `HeaderValue` metadata. Auth output
  producers use `common::auth_common::set_sensitive_request_header` for credentials,
  identity, claims, and cookies; do not convert a sensitive value back to String
  before forwarding it. See the [sensitive-header rule](../../04-review/security/mandatory-design/secret-header-values-must-be-sensitive.md).
- `set_upstream_uri()` / `set_upstream_host()` / `set_upstream_method()`
- `write_response_header()` / `write_response_body()` / `shutdown()`
- `set_ctx_var()` / `get_ctx_var()`
- `key_get()` / `key_set()`

If the plugin will call external HTTP services, prefer reusing:

- `edgion-gateway/src/plugins/http/common/http_client.rs`
- `edgion-gateway/src/plugins/http/common/auth_common.rs`

## `PluginLog` rules

Entry point:

- [../../../edgion-gateway/src/plugins/runtime/log.rs](../../../edgion-gateway/src/plugins/runtime/log.rs)

Keep the rules simple:

- Record results, not lengthy internal traces.
- Use short phrases ending with semicolons, e.g., `OK u=jack; `.
- Data-plane plugin callbacks must not use `tracing::` macros. Use concise `PluginLog` results in
  phases that receive a log, or bounded metrics/per-request aggregation where appropriate.
- Conditional skip (`cond_skip`) is written automatically by the conditional wrapper — do not invent your own format.
- **No PluginLog in the `UpstreamResponseBodyFilter` phase**: the trait signature deliberately omits the log parameter; the body phase covers data callbacks and no-trailers terminal completion, and per-callback logging blows up. See the full constraint in [05-plugin-system.md](05-plugin-system.md#-critical-constraint-no-pluginlog-in-body-phase-per-chunk-path-must-allocate-zero).

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

#### Building the `ErrResponse` body

`body` is `Option<bytes::Bytes>`, not `Option<String>`, so a local reply never
copies its bytes on the way to the wire. Pick the constructor by where the
bytes come from:

| Body source | Construct it as | Cost per request |
|---|---|---|
| A fixed literal | `Some(Bytes::from_static(b"..."))` | zero — no allocation |
| A configured value the plugin owns | store `Option<Bytes>` on the plugin struct at construction, then `self.body.clone()` | refcount bump |
| Built per request | `Some(Bytes::from(built_string))` | the one allocation you already made (`from` is a move) |
| No body at all | `None` | — |

Do **not** write `Some(Bytes::from("literal".to_string()))` — that allocates on
every request for a constant. And do not store a configured body as `String`
and convert it in `run_request`: that is the per-request deep copy this
representation exists to remove.

`None` and `Some(Bytes::new())` take different write paths — `None` ends the
response on the header write, `Some(empty)` follows it with a zero-length body
write — but both frame as `content-length: 0`, so the client sees the same
message. Prefer `None` when the reply has no body.

The body is raw bytes, so a binary local reply survives verbatim — but on a
gRPC route the body doubles as the `grpc-message` trailer, and a non-UTF-8 body
there falls back to the status-mapped default message instead of emitting
invalid header bytes.

**Runtime fail-safe**: a plugin that violates this convention receives a 502
Bad Gateway fallback from the runtime, and `PluginTerminateWithoutResponse`
appears in the ordered access-log `diagnostics` array as
`{"kind":"error","code":"PluginTerminateWithoutResponse"}`. Fix the plugin so
it writes its own response and the marker disappears.

## Project-specific rules

### No Consumer model

Do not design with the Kong / APISIX "Consumer -> Plugin" mindset.

In this repo:

- Credentials live mainly in Kubernetes Secrets.
- Plugins reference Secrets directly or use post-resolution runtime fields.
- Upstream identity is usually conveyed via request headers or ctx variables.

### Comments and logs in code use English

This is a general repo-wide convention; stay consistent with existing plugins.

## Failure policy for plugins with a runtime dependency

A plugin that calls a live per-request dependency must declare its failure behavior:

- **Auth purpose → fixed fail-close, no operator knob** (e.g. forward_auth,
  ldap_auth, openid_connect). Failing open is a security hole — do not add a
  degrade switch.
- **Non-auth → provide an explicit fail-open/fail-close knob, default fail-close.**
- Prefer a two-variant `Open`/`Close` enum over a `bool` for the knob: a `bool`
  named `failOpen` has polarity ambiguity (`false` = fail-close is a double
  negative). This is a preference — the existing `OnRedisFailure` enum,
  `failure_mode_allow: bool`, `fail_open: bool`, and dsl `error_policy` shapes
  are grandfathered and not being migrated.

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

Verifying initial state is not enough — add a dynamic scenario that exercises the "YAML update → runtime behavior change" path. Templates: remove a credential (BasicAuth) or switch an enum field (RateLimitLocal `onKeyMiss`).

- Look at existing scenarios to get started: `edgion-tests/src/client/dynamic/scenarios/basic_auth_dynamic.rs` / `rate_limit_local_dynamic.rs`.
- conf directory convention: `edgion-tests/integration/conf/EdgionPlugins/<Plugin>/DynamicTest/{initial,updates}/` (`load_conf.sh` automatically skips `updates/` and `delete/`).
- For the detailed steps, CLI entry, and registration sites: [../../05-testing/01-integration-testing.md "Dynamic (hot reload) testing" section](../../05-testing/01-integration-testing.md).

> **Pitfalls**: state-cached fields such as RateLimitLocal sliding-window counters can false-pass on hot reload (the old cache hits). Prefer verifying **logic-branch** changes such as "the entire ref is removed" or "an enum field is toggled" — do not just verify "password content modified" or "a rate value changed". (BasicAuth no longer keeps a result cache after bcrypt was dropped, so the structural-reload pattern is now mostly a regression-safety template for it rather than a cache-bypass workaround.)

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

- [docs/en/dev-guide/http-plugin-development.md](../../../docs/en/dev-guide/http-plugin-development.md)
- [docs/zh-CN/dev-guide/http-plugin-development.md](../../../docs/zh-CN/dev-guide/http-plugin-development.md) (Chinese mirror)
- [observability/00-access-log.md](../../03-coding/observability/00-access-log.md)
- [13-stream-plugin-dev.md](13-stream-plugin-dev.md)
- [14-link-sys-dev.md](14-link-sys-dev.md)
- [../../05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md)
