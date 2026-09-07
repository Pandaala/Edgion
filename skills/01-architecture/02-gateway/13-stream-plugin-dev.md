---
name: stream-plugin-development
description: Use when implementing or extending EdgionStreamPlugins, wiring Gateway or route-level stream-plugin annotations, or debugging the ConnectionFilter and TLSRoute plugin runtimes.
---
# Stream Plugin Development

Use this to answer questions like:

- How do I add or extend `EdgionStreamPlugins`?
- Where do `edgion.io/edgion-stream-plugins` annotations on Gateway / `TCPRoute` / `TLSRoute` actually wire into?
- What's the difference between Stage 1 `ConnectionFilter` and Stage 2 `TlsRoute` plugin runtimes?
- Why did the YAML change but the Gateway didn't hot-reload, or the runtime result is wrong?

## Read these real entry points first

- [../../../edgion-resources/src/resources/edgion_stream_plugins/mod.rs](../../../edgion-resources/src/resources/edgion_stream_plugins/mod.rs)
- [../../../edgion-resources/src/resources/edgion_stream_plugins/stream_plugins.rs](../../../edgion-resources/src/resources/edgion_stream_plugins/stream_plugins.rs)
- [../../../edgion-resources/src/resources/edgion_stream_plugins/tls_route_plugins.rs](../../../edgion-resources/src/resources/edgion_stream_plugins/tls_route_plugins.rs)
- [../../../edgion-resources/src/resource/meta/impls.rs](../../../edgion-resources/src/resource/meta/impls.rs)
- [../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs](../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs)
- [../../../edgion-gateway/src/plugins/stream/mod.rs](../../../edgion-gateway/src/plugins/stream/mod.rs)
- [../../../edgion-gateway/src/plugins/stream/stream_plugin_trait.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_trait.rs)
- [../../../edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs)
- [../../../edgion-gateway/src/plugins/stream/stream_plugin_store.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_store.rs)
- [../../../edgion-gateway/src/runtime/server/listener_builder.rs](../../../edgion-gateway/src/runtime/server/listener_builder.rs)
- [../../../edgion-gateway/src/routes/tcp/conf_handler_impl.rs](../../../edgion-gateway/src/routes/tcp/conf_handler_impl.rs)
- [../../../edgion-gateway/src/routes/tls/proxy.rs](../../../edgion-gateway/src/routes/tls/proxy.rs)

## Mental model

This pipeline is more than "write a trait".
The current real path in this repo is:

1. Define the `EdgionStreamPlugins` resource structure.
2. Initialize the runtime object during `ResourceMeta` preprocessing.
3. The Controller receives the resource, validates it, and writes status.
4. The Gateway `ConfigClient` syncs the resource into `StreamPluginStore`.
5. Gateway / `TCPRoute` / `TLSRoute` reference the resource through the `edgion.io/edgion-stream-plugins` annotation.
6. The data plane runs the runtime at different stages:
   - Stage 1: `ConnectionFilter` — pre-handshake, only IP / port and other connection context are available.
   - Stage 2: `TlsRoute` — after the TLS handshake and route match; SNI, matched route, mTLS state, etc. are available.

## Current code layout

```text
edgion-resources/src/resources/edgion_stream_plugins/
  mod.rs                    # CRD, spec/status, runtime initialization entry
  stream_plugins.rs         # Stage 1 plugin enum: EdgionStreamPlugin
  tls_route_plugins.rs      # Stage 2 plugin enum: TlsRouteStreamPlugin

edgion-gateway/src/plugins/stream/
  stream_plugin_trait.rs    # StreamPlugin / StreamContext / StreamPluginResult
  stream_plugin_runtime.rs  # Stage 1 runtime: builds the plugin chain from entries
  stream_plugin_store.rs    # Gateway global hot-reload store (ArcSwap)
  connection_filter_bridge.rs
  ip_restriction/           # ready reference implementation
  tls_route/                # Stage 2 runtime and trait
```

## The two execution stages

### Stage 1: ConnectionFilter

- The trait lives in [../../../edgion-gateway/src/plugins/stream/stream_plugin_trait.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_trait.rs).
- The context is `StreamContext`.
- Return value:
  - `StreamPluginResult::Allow`
  - `StreamPluginResult::Deny(String)`
- Typical binding sites:
  - Gateway listener level: `listener_builder.rs`.
  - `TCPRoute` level: `routes/tcp/conf_handler_impl.rs`.
- Suitable for:
  - IP allow/deny.
  - Rejecting connections as early as possible.
  - Logic that does not need SNI / route match / mTLS info.

### Stage 2: TlsRoute

- The Stage-2 runtime (`run_tls_route_plugins`) is invoked from **both** TLS proxies: the TLS-terminate proxy [../../../edgion-gateway/src/routes/tls/proxy.rs](../../../edgion-gateway/src/routes/tls/proxy.rs) and the TLS-passthrough proxy [../../../edgion-gateway/src/routes/tls/passthrough_proxy.rs](../../../edgion-gateway/src/routes/tls/passthrough_proxy.rs). Any binding-site change must touch both paths.
- `tls_route_plugin_runtime` runs first.
- Current code falls back from missing `tlsRoutePlugins` to `plugins`. This is a
  pre-release compatibility debt, not a Stage-2 contract. New code and fixtures
  must populate `tlsRoutePlugins`; do not depend on or extend the fallback, and
  remove it when the corresponding code change is in scope.
- Suitable for:
  - SNI-dependent logic.
  - Logic that depends on which `TLSRoute` was matched.
  - Logic that depends on `is_mtls` and other TLS context.

## Minimum checklist for adding a new stream plugin

### 1. Decide which stage it belongs to

- Depends only on IP / connection port: Stage 1.
- Depends on SNI / route / mTLS: Stage 2.
- Both: it can be added to both `EdgionStreamPlugin` and `TlsRouteStreamPlugin`.

### 2. Define or reuse the configuration struct

The current `IpRestriction` reuses `IpRestrictionConfig` from the HTTP plugin side directly:

- [../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/](../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/)

`IpRestrictionConfig` supports both inline rules (`allowlist`/`denylist`) and references to central
`IpList` EdgionConfigData objects via `allowRefs`/`denyRefs` (`Option<Vec<EdgionConfigDataRef>>`).
The gateway merges referenced IpLists into the same allow/deny pools as inline entries; a missing or
disabled referenced IpList causes the `allowRef` to fail closed (deny-all). The L4 variant is defined in
`edgion-resources/src/resources/edgion_stream_plugins/connection_ip_restriction_rules.rs`.

**Cross-namespace `allowRefs`/`denyRefs`:** when the referenced IpList EdgionConfigData is in a
different namespace, a Gateway-API `ReferenceGrant` must exist in the target namespace granting
`from {group: edgion.io, kind: EdgionStreamPlugins, namespace: <referrer>}` access to
`to {group: edgion.io, kind: EdgionConfigData}`. Alternatively, set `spec.visibility: Cluster`
on the target EdgionConfigData to grant access to all namespaces without a per-grant entry. A
denied cross-ns `allowRef` → deny-all fail-closed (safe); a denied `denyRef` → under-block.
See [15-edgion-config-data.md — Cross-Namespace Reference Grants](../../01-architecture/05-resources/15-edgion-config-data.md#cross-namespace-reference-grants).

If your stream plugin can also share configuration with the HTTP layer, prefer reuse; otherwise add a dedicated configuration.

## 3. Wire the new type into the resource enum

Stage 1:

- Modify [../../../edgion-resources/src/resources/edgion_stream_plugins/stream_plugins.rs](../../../edgion-resources/src/resources/edgion_stream_plugins/stream_plugins.rs).

Stage 2:

- Modify [../../../edgion-resources/src/resources/edgion_stream_plugins/tls_route_plugins.rs](../../../edgion-resources/src/resources/edgion_stream_plugins/tls_route_plugins.rs).

At minimum, add:

- The enum variant.
- `type_name()`.
- serde / schema compatibility.

## 4. Implement the runtime plugin

A Stage 1 plugin must implement:

```rust
#[async_trait]
pub trait StreamPlugin: Send + Sync {
    fn name(&self) -> &str;
    async fn on_connection(&self, ctx: &StreamContext) -> StreamPluginResult;
}
```

The most direct reference implementation:

- [../../../edgion-gateway/src/plugins/stream/ip_restriction/stream_ip_restriction.rs](../../../edgion-gateway/src/plugins/stream/ip_restriction/stream_ip_restriction.rs)

For Stage 2 plugins, look at:

- [../../../edgion-gateway/src/plugins/stream/tls_route/](../../../edgion-gateway/src/plugins/stream/tls_route/)

## 5. Register the construction logic in the runtime

Stage 1 runtime:

- [../../../edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs)

Stage 2 runtime:

- [../../../edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_runtime.rs](../../../edgion-gateway/src/plugins/stream/tls_route/tls_route_plugin_runtime.rs)

This is the actual "resource configuration → runtime plugin object" assembly point. Modifying only the enum without modifying the runtime means the Gateway will not execute your new plugin.

## 6. Confirm preprocessing and hot-reload paths need no fixes

Currently, runtime initialization for `EdgionStreamPlugins` is not done in the handler — it lives in:

- [../../../edgion-resources/src/resource/meta/impls.rs](../../../edgion-resources/src/resource/meta/impls.rs)

That is, the `ResourceMeta` preprocessing phase calls:

- `init_stream_plugin_runtime()`
- `init_tls_route_plugin_runtime()`

The Controller handler [../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs](../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs) currently mainly handles status, not runtime construction.

The Gateway-side hot-reload entries are:

- [../../../edgion-gateway/src/plugins/stream/stream_plugin_store.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_store.rs)
- [../../../edgion-gateway/src/conf_sync/conf_client/config_client.rs](../../../edgion-gateway/src/conf_sync/conf_client/config_client.rs)

## 7. Bind via YAML annotation and verify the reference path

The current key is:

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "policy-name"
```

On `Gateway`, the annotation accepts only an unqualified name and resolves it in the
Gateway namespace. TCPRoute, TLSRoute, and UDPRoute retain their separate `name` or
`namespace/name` contract.

Existing binding sites:

- Gateway listener level: `listener_builder.rs`.
- `TCPRoute`: `routes/tcp/conf_handler_impl.rs`.
- `TLSRoute`: `routes/tls/conf_handler_impl.rs` + `routes/tls/proxy.rs` (terminate) + `routes/tls/passthrough_proxy.rs` (passthrough). `run_tls_route_plugins` fires from both proxies.

If you only added the resource and the runtime but did not wire the route / listener side reference, the runtime will not fire either.

I have not currently found any code in the repo that handles this annotation specifically for `UDPRoute`, so do not assume UDP is wired the same as TCP/TLS.

## Minimal YAML example

### Define the resource

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-allow-localhost
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - name: localhost
            description: Local development clients
            cidrs:
              - 127.0.0.1/32
        # allowRefs: reference a central IpList EdgionConfigData (merged into the allow pool)
        # allowRefs:
        #   - name: corp-allowlist   # namespace defaults to edgion-test
```

### Reference it from a Gateway

Gateway-level attachments accept only an unqualified name and resolve it in the
Gateway namespace:

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: tcp-allow-localhost
```

### Reference it from a `TCPRoute` / `TLSRoute` / `UDPRoute`

Route-level attachments retain their separate `name` or `namespace/name` contract:

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: edgion-test/tcp-allow-localhost
```

## ConnectionRateLimit (L4 connection-rate gate)

`EdgionStreamPlugin::ConnectionRateLimit` bounds the **rate** of new downstream
TCP/TLS connections. Two-stage admission per accepted connection (pre-TLS):

1. **Stage 1 — local per-listener gate** (token bucket, process-global keyed by
   listener port; survives config hot-reload). No external dependency, so it
   stays available under flood when Redis is the first thing to fall over.
2. **Stage 2 — optional Redis** (sliding/fixed/token-bucket via the shared
   `RedisRateLimiter` core) for `perSourceIp` and, when `redisDistributed: true`,
   a fleet-wide `perListener` cap. Accept-path Redis calls are capped by the
   configurable **`redisTimeout`** (default `100ms`, hard-capped at `1s`) so a
   slow Redis cannot pin downstream fds. `perSourceIp` has no local backstop, so
   every accepted connection on that dimension must reach Redis within
   `redisTimeout`.

Per-dimension `onRedisFailure` defaults differ on purpose:
`perListener` defaults to `Deny` (fail-close — the process backstop must not
vanish under attack); `perSourceIp` defaults to `Allow` (fail-open — a Redis
blip must not blanket-block legitimate sources). Both are overridable.

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata: { name: tcp-conn-rate, namespace: edgion-test }
spec:
  plugins:
    - type: ConnectionRateLimit
      config:
        # redisRef is required only when a Redis-backed dimension is on
        # (perSourceIp, or perListener with redisDistributed: true).
        redisRef: "default/shared-redis"
        algorithm: SlidingWindow            # default
        redisTimeout: "200ms"               # default 100ms; hard-capped at 1s
        perListener:
          rate: 10000
          interval: "1s"
          redisDistributed: false           # local-only by default
          # onRedisFailure: Deny             # default
        perSourceIp:
          rate: 50
          interval: "1s"
          # onRedisFailure: Allow            # default
```

### Coverage limits (read before relying on this for fd-exhaustion defense)

- **Opt-in only.** Requires the `edgion.io/edgion-stream-plugins` annotation
  plus an `EdgionStreamPlugins` resource. A bare gateway has no admission
  control.
- **Does NOT cover** the admin API, probe API, or the controller↔gateway gRPC
  link — those are not Pingora listeners (separate axum servers / outbound
  client) and stream plugins cannot reach them.
- **Does NOT cover UDP** (no `ConnectionFilter` is attached; UDP is
  connectionless).
- **Bounds connection rate, NOT concurrent fds.** A slowloris-style adversary
  that opens connections slowly and never closes them can still exhaust fds
  without tripping any rate. Defending against that needs concurrent-count
  limiting (blocked on a Pingora connection-close hook) and/or handshake/idle
  timeouts + OS-level limits. Pair this plugin with those, not as a substitute.
- The source task's process-wide fd-exhaustion safety net remains open after
  this plugin — see
  `docs/superpowers/specs/2026-06-12-connection-rate-limit-stream-plugin-design.md`.

Reject metric: `edgion_connection_rate_limit_rejected_total` with labels
`dimension={listener|source_ip}` and `stage={local|redis}` (fixed, low
cardinality) — fires only on **Deny**.

Redis-failure metric: `edgion_connection_rate_limit_redis_failure_total` with
labels `dimension={listener|source_ip}` and `cause={unavailable|runtime}` —
fires on **every** Redis failure regardless of the Allow/Deny outcome, so
fail-open is observable (`unavailable` = client missing / not healthy, classified
via an explicit `client.healthy()` pre-check before the core check; `runtime` =
command error or `redisTimeout` elapse).

User-facing config reference (canonical): see
`docs/en/user-guide/tcp-route/stream-plugins.md` — "ConnectionRateLimit"
(`redisTimeout`, the inbound-fd vs outbound-pool rationale, the `perSourceIp`
no-backstop caveat, and the PromQL alert example).

## Key differences from HTTP plugins

| Dimension | Stream plugin | HTTP plugin |
|-----------|---------------|-------------|
| Execution layer | TCP/TLS connection layer | HTTP request/response layer |
| Main context | `StreamContext` / `TlsRouteContext` | `PluginSession` |
| Failure action | Reject the connection | Return an HTTP response or terminate the request |
| Binding | The `edgion.io/edgion-stream-plugins` annotation references the resource | The `EdgionPlugins` runtime chain on the Route/Gateway |
| Hot-reload source | `StreamPluginStore` | HTTP plugin runtime / store |

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

Cover these three areas in priority order:

1. Whether the resource status is correct.
2. Whether the plugin actually fires after Gateway / `TCPRoute` / `TLSRoute` references it.
3. Whether hot-reload takes effect after `EdgionStreamPlugins` is updated / deleted, and whether the old configuration leaves residue.

References for existing samples:

- [../../../edgion-tests/integration/conf/Gateway/StreamPlugins/](../../../edgion-tests/integration/conf/Gateway/StreamPlugins/)
- [../../../edgion-tests/integration/conf/TCPRoute/StreamPlugins/](../../../edgion-tests/integration/conf/TCPRoute/StreamPlugins/)
- [../../../edgion-tests/integration/conf/TLSRoute/StreamPlugins/](../../../edgion-tests/integration/conf/TLSRoute/StreamPlugins/)

### Item 3 entry: the dynamic scenario framework

Hot-reload regression must not stop at "is the new value in the store" — make the connection / route actually exercise the new behavior. The `edgion-tests/src/client/dynamic/` framework supports N-phase state-machine assertions; the built-in scenarios are HTTP-level (`gateway_dynamic`, `dsl_dynamic`, `basic_auth_dynamic`, `rate_limit_dynamic`); stream plugins **don't have a scenario template yet** — `ScenarioCtx` / `assert_phase` is scheduled by `TestSuite`, so in principle writing a `TestSuite` that emits TCP/TLS traffic is enough to reuse the entire orchestration.

How to write it: follow the structure of `edgion-tests/src/client/dynamic/scenarios/basic_auth_dynamic.rs`, replacing the phase suite with a stream-aware one (verifying connection accept/deny, TLS handshake behavior, etc.). For detailed registration sites and CLI entry, see [../../05-testing/01-integration-testing.md "Dynamic (hot reload) testing" section](../../05-testing/01-integration-testing.md).

## Review checklist

- Is the new plugin Stage 1, Stage 2, or both?
- Are the enum, runtime, and module exports all wired?
- Is an existing configuration struct reused instead of copying a near-duplicate one?
- After `ResourceMeta` preprocessing, are the runtime fields actually initialized?
- Does Gateway annotation parsing accept only a same-namespace `name`, while route annotation parsing still supports both `name` and `namespace/name`?
- After updating / deleting the resource, does the Gateway store hot-reload correctly?

## Related

- [00-annotations-overview.md](../../02-features/05-annotations/00-annotations-overview.md)
- [11-conf-handler-guidelines.md](11-conf-handler-guidelines.md)
- [12-edgion-plugin-dev.md](12-edgion-plugin-dev.md)
- [01-integration-testing.md](../../05-testing/01-integration-testing.md)
