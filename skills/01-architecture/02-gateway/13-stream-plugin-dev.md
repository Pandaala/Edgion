---
name: stream-plugin-development
description: Use when implementing or extending EdgionStreamPlugins, wiring Gateway or route-level stream-plugin annotations, or debugging the connection-context and TLSRoute plugin runtimes.
---
# Stream Plugin Development

Use this to answer questions like:

- How do I add or extend `EdgionStreamPlugins`?
- Where do `edgion.io/edgion-stream-plugins` annotations on Gateway / `TCPRoute` / `TLSRoute` / `UDPRoute` actually wire into?
- What's the difference between the Stage 1 connection-context runtime and the Stage 2 `TlsRoute` runtime?
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
- [../../../edgion-gateway/src/routes/udp/route_unit.rs](../../../edgion-gateway/src/routes/udp/route_unit.rs)
- [../../../edgion-gateway/src/routes/udp/edgion_udp.rs](../../../edgion-gateway/src/routes/udp/edgion_udp.rs)

## Mental model

This pipeline is more than "write a trait".
The current real path in this repo is:

1. Define the `EdgionStreamPlugins` resource structure.
2. Define serializable stage entries; the schema does not contain runtime objects.
3. The Controller receives the resource, validates it, and writes status.
4. The Gateway `ConfigClient` syncs the source through `StreamPluginStore` into the canonical `PluginPublisher`, which builds immutable class-scoped runtime views for demanded attachments.
5. Gateway / `TCPRoute` / `TLSRoute` / `UDPRoute` reference the resource through the `edgion.io/edgion-stream-plugins` annotation.
6. The data plane runs the runtime at different stages:
   - Stage 1: connection-context runtime — only IP / port and other connection context are available. Gateway attachments use Pingora's pre-handshake `ConnectionFilter`; TCPRoute runs it per connection, while UDPRoute runs it when admitting a new UDP session.
   - Stage 2: `TlsRoute` — after route matching; Terminate supplies negotiated TLS identity, while Passthrough supplies inspected SNI before forwarding the handshake.

## Current code layout

```text
edgion-resources/src/resources/edgion_stream_plugins/
  mod.rs                    # CRD, spec/status, serializable stage entries
  stream_plugins.rs         # Stage 1 plugin enum: EdgionStreamPlugin
  tls_route_plugins.rs      # Stage 2 plugin enum: TlsRouteStreamPlugin

edgion-gateway/src/plugins/stream/
  stream_plugin_trait.rs    # StreamPlugin / StreamContext / StreamPluginResult
  stream_plugin_runtime.rs  # Stage 1 runtime: builds the plugin chain from entries
  stream_plugin_store.rs    # Conf-sync/source adapter into PluginPublisher
  connection_filter_bridge.rs
  ip_restriction/           # ready reference implementation
  tls_route/                # Stage 2 runtime and trait
```

## The two execution stages

### Stage 1: connection-context runtime

- The trait lives in [../../../edgion-gateway/src/plugins/stream/stream_plugin_trait.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_trait.rs).
- The context is `StreamContext`.
- Return value:
  - `StreamPluginResult::Allow`
  - `StreamPluginResult::Deny(String)`
- Typical binding sites:
  - Gateway listener level for Pingora `Service` listeners: `listener_builder.rs`. UDP listeners are separate Tokio tasks and do not use this `ConnectionFilter`.
  - `TCPRoute` level: `routes/tcp/conf_handler_impl.rs`.
  - `UDPRoute` level: `routes/udp/route_unit.rs` resolves the key and `routes/udp/edgion_udp.rs` checks it when creating a session.
- Suitable for:
  - IP allow/deny.
  - Rejecting connections as early as possible.
  - Logic that does not need SNI / route match / mTLS info.
- Gateway listener admission captures one publication snapshot and obtains the
  physical listener's effective `ConnectionView`. The publisher compares every
  serving Gateway before exposing a common view; disagreement or unavailable
  policy fails closed. A ready admission pins its `CompiledStreamPlugins` Arc.
  The fatal PROXY-policy guard runs before the bridge, and inbound PROXY parsing
  runs after it. The Stage-1 peer is therefore the physical pre-PROXY
  `DirectPeerIp`; IPv4-mapped IPv6 is canonicalized to IPv4 without changing
  peer ownership.

### Stage 2: TlsRoute

- The Stage-2 runtime (`run_tls_route_plugins`) is invoked from **both** TLS proxies: the TLS-terminate proxy [../../../edgion-gateway/src/routes/tls/proxy.rs](../../../edgion-gateway/src/routes/tls/proxy.rs) and the TLS-passthrough proxy [../../../edgion-gateway/src/routes/tls/passthrough_proxy.rs](../../../edgion-gateway/src/routes/tls/passthrough_proxy.rs). Any binding-site change must touch both paths.
- TLSRoute executes only `tls_route_plugin_runtime`, never the connection-stage runtime.
- If `tlsRoutePlugins` is absent (or null) and raw `plugins` is nonempty, the
  attachment fails closed, including disabled or Bypass Stage-1 entries. This is
  a stage-binding error, not validation of disabled entries.
- Explicit `tlsRoutePlugins: []`, or a Stage-2 chain emptied by disabled/Bypass
  entries, is a no-op. Both fields absent/empty is also a no-op. A missing
  referenced resource still fails closed. Enabled Stage-2 Block entries deny.
- Move route-owned IP restrictions to `tlsRoutePlugins`. Connection-only
  plugins such as GeoIP and ConnectionRateLimit belong on the Gateway listener
  (or TCP/UDP route), not on TLSRoute. Gateway and TLSRoute stages remain separate.
- Each new route admission resolves and pins the current compiled view from one
  publication; existing connections are not re-authorized when the resource
  changes.
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

Stage 1 IpRestriction uses `ConnectionIpRestrictionRules`; TLS-route IpRestriction reuses HTTP `IpRestrictionConfig`. Both embed the canonical reference-only ACL set (`allowRefs`/`denyRefs`). Cold construction borrows the actual owning field, validates source/slot/domain permission proof, and creates a shared `CompiledIpAcl` with ordered keys. One decision-local ConfigData snapshot supplies lazy matcher references; unavailable ACL sources are omitted. A nonempty authored allow side preserves whitelist intent even when every target is unavailable. No plugin owns CIDRs or local matchers.

**Cross-namespace `allowRefs`/`denyRefs`:** when the referenced IpList EdgionConfigData is in a
different namespace, a Gateway-API `ReferenceGrant` must exist in the target namespace granting
`from {group: edgion.io, kind: EdgionStreamPlugins, namespace: <referrer>}` access to
`to {group: edgion.io, kind: EdgionConfigData}`. Alternatively, set `spec.visibility: Cluster`
on the target EdgionConfigData to grant access to all namespaces without a per-grant entry. A
denied cross-ns `allowRef` → deny-all fail-closed (safe); a denied `denyRef` → under-block.
See [15-edgion-config-data.md — Cross-Namespace Reference Grants](../../01-architecture/05-resources/15-edgion-config-data.md#cross-namespace-reference-grants).

If your stream plugin can also share configuration with the HTTP layer, prefer reuse; otherwise add a dedicated configuration.

## 3. Wire the new type into the resource enum

Adding a connection or TLSRoute plugin type requires no CRD whitelist update. Both `type`
fields are open strings. Unknown types become schema-hidden `Unsupported` sentinels; the
Controller reports `Accepted=False`, and every demanded attachment of that source denies
connections, including when the entry is disabled or a policy would bypass it. Keep explicit
exhaustive arms for real plugins. Known config validation remains typed and unchanged.


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

Gateway `StreamPluginStore` forwards immutable source inputs to the canonical
`PluginPublisher`, which publishes class-scoped views atomically.
`StreamPluginGeneration` is the sole production resource-to-executable-view
construction and orchestration path: it constructs only enabled, permitted entries
demanded by attached class/stage views and shares their cached instances.
`ResourceMeta` and Controller handlers never build Gateway runtime objects.
The Controller resolves references, validates enabled source configuration and
writes status; it does not stamp execution policy into transported entries.

`EdgionGatewayConfig.spec.pluginPolicy` controls `stream/` and `tlsRoute/` names
through GatewayClass parametersRef. Block closes new applicable admissions;
Bypass never constructs or runs the restricted entry for that scope. Listener
admission compares every serving Gateway candidate's effective connection view,
allowing one execution only when those views are equivalent. The bridge captures
one publication snapshot, reads the physical listener view, fails closed on
`Unavailable`, and pins the selected `CompiledStreamPlugins`. TCP/UDP/TLS-route
admission uses the matched logical Gateway. Existing admitted sessions remain
unchanged; policy edits do not restart listeners.

Hot-reload source adapters live in
[stream_plugin_store.rs](../../../edgion-gateway/src/plugins/stream/stream_plugin_store.rs)
and [publication](../../../edgion-gateway/src/plugins/publication/mod.rs).
See [scoped publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md)
for retention, constructor ownership and verification limits.

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

- Gateway listener level for Pingora `Service` listeners: `listener_builder.rs`; this does not include UDP listeners.
- `TCPRoute`: `routes/tcp/conf_handler_impl.rs`.
- `TLSRoute`: `routes/tls/conf_handler_impl.rs` + `routes/tls/proxy.rs` (terminate) + `routes/tls/passthrough_proxy.rs` (passthrough). `run_tls_route_plugins` fires from both proxies.
- `UDPRoute`: `routes/udp/route_unit.rs` compiles the dynamic store key and `routes/udp/edgion_udp.rs` runs `spec.plugins` before creating a new session.

If you only added the resource and the runtime but did not wire the route / listener side reference, the runtime will not fire either.

UDPRoute does not use Pingora's `ConnectionFilter`: its listener is an independent
Tokio task. The route annotation is nevertheless supported. It executes the same
connection-context runtime once when admitting a new source-address session, before
backend selection; established-session packets take the fast path and are not
re-authorized. A missing referenced resource fails closed by dropping the datagram.
Only `spec.plugins` applies because UDP has no TLS stage.

## Minimal YAML example

### Define the resource

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-tcp-allow-localhost-allow-1
  namespace: edgion-test
spec:
  data:
    type: IpList
    config:
      items:
      - name: localhost
        description: Local development clients
        cidrs:
        - 127.0.0.1/32
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-allow-localhost
  namespace: edgion-test
spec:
  plugins:
  - type: IpRestriction
    config:
      allowRefs:
      - name: example-tcp-allow-localhost-allow-1
        namespace: edgion-test
```

### Reference it from a Gateway

Gateway-level attachments accept only an unqualified name and resolve it in the
Gateway namespace:

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: tcp-allow-localhost
```

### Reference it from a `TCPRoute` / `UDPRoute`

Route-level attachments retain their separate `name` or `namespace/name` contract:

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: edgion-test/tcp-allow-localhost
```

### Reference a TLSRoute-stage resource

TLSRoute requires its own stage declaration; do not attach the Stage-1-only
resource above. The route annotation uses the same name-resolution syntax.

```yaml
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: example-tls-allow-localhost-allow-2
  namespace: edgion-test
spec:
  data:
    type: IpList
    config:
      items:
      - name: localhost
        cidrs:
        - 127.0.0.1/32
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tls-allow-localhost
  namespace: edgion-test
spec:
  tlsRoutePlugins:
  - type: IpRestriction
    config:
      defaultAction: deny
      allowRefs:
      - name: example-tls-allow-localhost-allow-2
        namespace: edgion-test
```

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: edgion-test/tls-allow-localhost
```

## ConnectionRateLimit (L4 connection-rate gate)

`EdgionStreamPlugin::ConnectionRateLimit` bounds the **rate** of new downstream
TCP/TLS connections or UDP sessions. Listener attachments run pre-TLS on Pingora
`Service` listeners; TCP/UDP route attachments run at their owning connection/session
lifecycle. The checks are:

1. **Local per-listener gate** (token bucket, process-global keyed by plugin
   entry scope and listener port; survives config hot-reload). No external dependency, so it
   stays available under flood when Redis is the first thing to fall over.
2. **Optional Redis** (sliding/fixed/token-bucket via the shared
   `RedisRateLimiter` core) for `perSourceIp` and, when `redisDistributed: true`,
   a fleet-wide `perListener` cap. Accept-path Redis calls are capped by the
   configurable **`redisTimeout`** (default `100ms`, hard-capped at `1s`) per
   Redis dimension. Multiple dimensions or entries can accumulate waits.
   `perSourceIp` has no local fallback; missing/unhealthy clients apply the
   failure policy immediately. Listener-attached Redis checks still occupy
   an accept task while awaiting a response. Local filtering reduces that load
   but does not eliminate the wait. See the
   [accepted admission tradeoff](../../04-review/listeners/intentional-design/bounded-serial-connection-admission.md).

Per-dimension `onRedisFailure` defaults differ on purpose:
`perListener` defaults to `Deny` (fail-close — the process backstop must not
vanish under attack); `perSourceIp` defaults to `Allow` (fail-open — a Redis
blip must not blanket-block legitimate sources). Both are overridable.

The local runtime binds `LocalGateBinding` once during valid plugin construction.
The existing process-lifetime registry owns an `EntryGates` per entry scope;
connection hits resolve a sparse `u16` port map under that entry's read lock,
release it, and enter the bucket mutex. Only first use allocates a port slot.
The registry remains non-evicting budget state, not a general-purpose cache.

`LocalRateGate` publishes an unactivated `Mutex<Option<GateState>>` slot. Its
first locked admission initializes the generation, parameters, full balance and
monotonic timestamp together. Every subsequent admission atomically settles old
refill, adopts a higher runtime generation if encountered, migrates capacity,
and consumes one token. An old runtime cannot restore its parameters. Equal
parameters preserve the refill baseline; changed parameters start a new baseline
after old-rate settlement. No gate lock spans Redis work or observability.

Generations follow the production serial conf-sync construction order. Activation
is lazy per port, not eager at store publication. Constructed but unexecuted
generations have no budget effect; intermediate unobserved generations are skipped.
Older pinned admissions consume the newest activated local budget while retaining
their original Redis settings. This does not provide a local-plus-Redis transaction.
See the [algorithm contract](../../12-algorithms/local-connection-token-bucket.md)
and [local transition regression guard](../../04-review/rate-limit/regression-guards/local-connection-admission-transitions.md).

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
- **Gateway listener attachment does NOT cover UDP.** UDP listeners do not use
  Pingora's `Service` or `ConnectionFilter`. To protect UDP admission, attach the
  resource to `UDPRoute`; that path checks `spec.plugins` once before creating a
  source-address session, not once per datagram.
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
| Hot-reload source | `StreamPluginStore` source adapter → canonical `PluginPublisher` | HTTP source adapter → canonical `PluginPublisher` |

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
2. Whether the plugin actually fires after Gateway / `TCPRoute` / `TLSRoute` / `UDPRoute` references it.
3. Whether hot-reload takes effect after `EdgionStreamPlugins` is updated / deleted, and whether the old configuration leaves residue.

References for existing samples:

- [../../../edgion-tests/integration/conf/Gateway/StreamPlugins/](../../../edgion-tests/integration/conf/Gateway/StreamPlugins/)
- [../../../edgion-tests/integration/conf/TCPRoute/StreamPlugins/](../../../edgion-tests/integration/conf/TCPRoute/StreamPlugins/)
- [../../../edgion-tests/integration/conf/TLSRoute/StreamPlugins/](../../../edgion-tests/integration/conf/TLSRoute/StreamPlugins/)
- [../../../edgion-tests/integration/conf/UDPRoute/StreamPlugins/](../../../edgion-tests/integration/conf/UDPRoute/StreamPlugins/)

### Item 3 entry: the dynamic scenario framework

Hot-reload regression must exercise real traffic, not just store contents.
Use `edgion-tests/src/client/dynamic/scenarios/tls_route_plugins_reload.rs` as the
TLS example: shared Terminate/Passthrough listeners, independent healthy controls,
allow/deny generation transitions, and cleanup. `ScenarioCtx::wait_phase` polls
for the data-plane result; fixed sleeps are not publication barriers.

For registration and CLI usage, see the
[dynamic testing guide](../../05-testing/01-integration-testing.md).

## Review checklist

- Is the new plugin Stage 1, Stage 2, or both?
- Are the enum, runtime, and module exports all wired?
- Is an existing configuration struct reused instead of copying a near-duplicate one?
- Does `PluginPublisher` select the scoped policy and demanded stages before `StreamPluginGeneration` builds the executable view?
- Does Gateway annotation parsing accept only a same-namespace `name`, while route annotation parsing still supports both `name` and `namespace/name`?
- After updating / deleting the resource, does the Gateway store hot-reload correctly?

## Related

- [00-annotations-overview.md](../../02-features/05-annotations/00-annotations-overview.md)
- [11-conf-handler-guidelines.md](11-conf-handler-guidelines.md)
- [12-edgion-plugin-dev.md](12-edgion-plugin-dev.md)
- [01-integration-testing.md](../../05-testing/01-integration-testing.md)

### Active ConnectionRateLimit Redis ownership

The immutable Stream generation lends its actual ConnectionRateLimit config
through `refs::stream_plugin_fields` before entry-cache reuse. Source identity
is checked only when `needs_redis()` is true, after disabled/Block/Bypass policy
handling. An invalid active source denies even with onRedisFailure Allow;
local-only unused redisRef values retain their existing behavior. The canonical
config validator owns the Unicode-alphanumeric/-/_ target grammar. No Grant or
second IP/LinkSys prepared catalogue is introduced, and runtime Redis lookups
remain live. Standalone low-level config constructors are not owner-bound APIs.
