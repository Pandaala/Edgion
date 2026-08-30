---
name: controller-resource-processor
description: ResourceProcessor pipeline — ProcessorHandler trait, ControllerRuntime-backed HandlerContext, processing flow, handler inventory, reference managers, and RawStore lookup.
---

# ResourceProcessor Pipeline

ResourceProcessor is the Controller's core processing engine. Each resource type owns a `ResourceProcessor<K>` with a `ServerCache<K>`, Workqueue, handler, and shared `Arc<ControllerRuntime>`.

## ProcessorHandler\<K\> trait

Each resource type implements this trait to define its own processing logic. Handlers are stateless — all state is owned by the ResourceProcessor.

```rust
#[async_trait]
pub trait ProcessorHandler<K>: Send + Sync
where
    K: Resource + Clone + Send + Sync + 'static,
{
    /// Resource filter — returning false skips the resource. Default: accept all.
    fn filter(&self, obj: &K) -> bool { true }

    /// Clean metadata (remove managedFields, irrelevant annotations, etc.). Default: no-op.
    fn clean_metadata(&self, obj: &mut K, ctx: &HandlerContext) {}

    /// Resource validation — returns a list of warnings (does not interrupt processing). Default: no validation.
    fn validate(&self, obj: &K, ctx: &HandlerContext) -> Vec<String> { vec![] }

    /// Preparse: build runtime structures, validate plugin config, etc. Errors are merged into the validate error list automatically.
    fn preparse(&self, obj: &mut K, ctx: &HandlerContext) -> Vec<String> { vec![] }

    /// Parse / preprocess: register Secret references, resolve external references, etc. Returns Continue or Skip.
    async fn parse(&self, obj: K, ctx: &HandlerContext) -> ProcessResult<K> { ProcessResult::Continue(obj) }

    /// Delete cleanup: clear SecretRefManager registrations, etc.
    async fn on_delete(&self, obj: &K, ctx: &HandlerContext) {}

    /// Post-change processing: invoked before saving to cache, used for cascading requeue.
    async fn on_change(&self, obj: &K, ctx: &HandlerContext) {}

    /// Invoked after the init LIST phase completes; used for replace_all and other full-sync operations.
    fn on_init_done(&self, ctx: &HandlerContext) {}

    /// Update resource status fields (Accepted, ResolvedRefs, and other Gateway API conditions).
    fn update_status(&self, obj: &mut K, ctx: &HandlerContext, validation_errors: &[String]) {}
}
```

The `ProcessResult<K>` enum has two variants: `Continue(K)` (continue processing) and `Skip { reason }` (drop the resource).

## HandlerContext

The context object passed to handler methods. It provides the following shared resources:

| Field | Type | Purpose |
|---|---|---|
| `runtime` | `Arc<ControllerRuntime>` | Owns ProcessorRegistry, SecretStore, reference managers, and indexes; accessed through methods |
| `metadata_filter` | `Option<Arc<MetadataFilterConfig>>` | Metadata cleanup-rule configuration |
| `namespace_filter` | `Option<Arc<Vec<String>>>` | Namespace allowlist filter |
| `trigger_chain` | `TriggerChain` | Cascade trigger chain, used for cycle detection during requeue |
| `max_trigger_cycles` | `usize` | Max number of times the same (kind, key) is allowed to repeat in the trigger chain (default 5) |
| `max_trigger_depth` | `usize` | Maximum total trigger-chain depth (default 20) |

Key methods:

- `runtime()` — access the per-ConfCenter state owner and its stores/indexes.
- `secret_ref_manager()` — access the runtime-owned Secret dependency manager.
- `requeue(kind, key)` — enforce cycle/depth limits, then dispatch through `runtime.processor_registry.requeue_with_chain()`.
- `clean_metadata(obj)` — clean up resource metadata according to the `metadata_filter` configuration.

## 11-Step Processing Flow

The `process_resource()` method implements the full processing pipeline:

```
Input: original resource obj
  │
  ├─ 1. namespace filter    Check the namespace allowlist; Skip if not allowed
  ├─ 2. handler filter      Call handler.filter(); Skip if it returns false
  ├─ 3. clean metadata      Run ctx.clean_metadata() first (global config), then handler.clean_metadata() (custom cleanup)
  ├─ 4. validate            Call handler.validate(); collect validation errors (recorded as warnings, does not interrupt)
  ├─ 5. preparse            Call handler.preparse(); errors are merged into the validation error list
  ├─ 6. parse               Call handler.parse(); returns Continue(obj) or Skip
  ├─ 7. extract old status  Extract the old status for later comparison
  │                          - K8s mode: extracted from the object itself
  │                          - FileSystem mode: uses the passed-in existing_status_json
  ├─ 8. update status       Call handler.update_status(); set Gateway API conditions
  ├─ 9. check change        Semantically compare new and old status to determine whether status really changed
  ├─ 10. on_change          Call handler.on_change(); perform cascading actions (requeue, etc.)
  │                          Also invoked during the init phase; requeue items are buffered by the Workqueue and processed after InitDone
  └─ 11. save to cache      Init phase uses InitAdd (synchronous); runtime uses EventUpdate
                             After saving, ServerCache notifies gRPC watchers

Output: WorkItemResult::Processed { obj, status_changed }
        or WorkItemResult::Skipped
```

The delete flow (`process_delete`) is simpler: call `handler.on_delete()` to clean up reference relationships, then remove from the cache.

## Handler Implementations

Handler coverage follows `edgion-resources/src/resource/defs.rs` and
`handlers/mod.rs`; avoid copying a fixed total into new guidance.

### Gateway API Routes (5)

| Handler | Resource | Key processing |
|---|---|---|
| `HttpRouteHandler` | HTTPRoute | Hostname resolution, Service backend reference registration, cross-namespace validation, gateway_route_index registration |
| `GrpcRouteHandler` | GRPCRoute | Same as HTTPRoute, plus gRPC method matching |
| `TcpRouteHandler` | TCPRoute | Service backend reference registration, attached_route_tracker |
| `TlsRouteHandler` | TLSRoute | Same as TCPRoute, plus gateway_route_index registration |
| `UdpRouteHandler` | UDPRoute | Service backend reference registration, attached_route_tracker |

### Gateway API Core (4)

| Handler | Resource | Key processing |
|---|---|---|
| `GatewayHandler` | Gateway | Listener port-conflict detection, hostname/port cache update, route requeue |
| `GatewayClassHandler` | GatewayClass | Controller-name validation, Accepted/SupportedVersion conditions |
| `BackendTlsPolicyHandler` | BackendTLSPolicy | Backend TLS policy validation |
| `ReferenceGrantHandler` | ReferenceGrant | Cross-namespace authorization-change notifications, CrossNsRevalidationListener |

### K8s Core Resources (4)

| Handler | Resource | Key processing |
|---|---|---|
| `ServiceHandler` | Service | Requeue dependent routes on change |
| `EndpointSliceHandler` | EndpointSlice | Updates endpoint state and requeues dependent resources |
| `SecretHandler` | Secret | Cascading requeue of dependent resources on change |
| `ConfigmapHandler` | ConfigMap | Updates CA/config references and requeues dependents |

### Edgion Custom Resources

| Handler | Resource | Key processing |
|---|---|---|
| `EdgionGatewayConfigHandler` | EdgionGatewayConfig | Gateway-associated configuration |
| `EdgionPluginsHandler` | EdgionPlugins | Plugin configuration validation, Secret reference registration |
| `EdgionStreamPluginsHandler` | EdgionStreamPlugins | Stream-plugin configuration validation |
| `EdgionTlsHandler` | EdgionTls | TLS configuration, Secret reference registration, gateway_route_index |
| `EdgionAcmeHandler` | EdgionAcme | ACME certificate configuration, Secret references |
| `EdgionConfigDataHandler` | EdgionConfigData | Plugin metadata |
| `LinkSysHandler` | LinkSys | LinkSys configuration |
| `EdgionBackendTrafficPolicyHandler` | EdgionBackendTrafficPolicy | Policy validation, precedence/conflict status, and affected-route requeue |

### Shared Utility Modules

| Module | Purpose |
|---|---|
| `route_utils` | Common route logic: Service backend reference registration (`register_service_backend_refs`), parentRef parsing |
| `hostname_resolution` | Hostname resolution: hostname intersection between Route and Gateway Listener |

## Reference Managers

All reference managers are based on a unified generic `BidirectionalRefManager<V>` that maintains a bidirectional many-to-many index:

- **Forward index**: `source_key → HashSet<V>` — which resources reference this source
- **Reverse index**: `value_key → HashSet<source_key>` — which sources a given resource depends on

Core operations: `add_ref`, `remove_ref`, `clear_value_refs` (clear-then-add on delete/update), `get_refs` (look up dependents).

### Concrete Managers

| Manager | Type alias | Purpose |
|---|---|---|
| `SecretRefManager` | `BidirectionalRefManager<ResourceRef>` | Secret dependency tracking (Secret → Gateway, EdgionTls, EdgionPlugins, EdgionAcme) |
| `ServiceRefManager` | `BidirectionalRefManager<ResourceRef>` | Service backend dependencies (Service → all route types) |
| `CrossNamespaceRefManager` | `BidirectionalRefManager<ResourceRef>` | Cross-namespace references (ReferenceGrant → referencing resources) |
| `GatewayRouteIndex` | Custom bidirectional index | Gateway ↔ Route mapping, with hostname/port change-detection cache |
| `AttachedRouteTracker` | Custom struct | Tracks Route → parent Gateway attachment state (used for the `attachedRoutes` count in Gateway status) |

### ReferenceGrant System

- `ReferenceGrantStore` — stores all ReferenceGrant resources
- `CrossNamespaceValidator` — validates whether a cross-namespace reference is authorized
- `CrossNsRevalidationListener` — listens for ReferenceGrant changes and requeues affected resources

## Status Utilities

### status_utils

Provides building utilities for standard Gateway API conditions:

- `accepted_condition(generation)` — build an Accepted=True condition
- `resolved_refs_condition(generation)` — build a ResolvedRefs=True condition
- `condition_true/condition_false` — build arbitrary conditions
- `update_condition(conditions, new_condition)` — update the condition list (merging conditions of the same type)
- `set_parent_conditions` / `set_parent_conditions_full` — set parentRef status of a route

Error types: `AcceptedError`, `ResolvedRefsError` are used to build failure conditions.

### status_diff

Semantic change detection: when comparing new and old status, ignores non-semantic fields like `lastTransitionTime` to avoid unnecessary K8s API writes. Use `status_semantically_changed(kind, old_json, new_json)` to decide whether to persist.

## RawStore and ServerCache — Choose the Correct Cross-resource View

`ServerCache` only stores resources that pass `handler.filter()` (e.g., `GatewayHandler` filters by `gatewayClassName`). When a Route needs to look up a Gateway that does not belong to this controller (parentRef can point to any Gateway), `ServerCache` cannot find it, leading to incorrect Route status calculations.

`RawStore<K>` bypasses the handler filter and provides O(1) single-key lookup:

```rust
pub trait RawStore<K>: Send + Sync {
    fn get(&self, key: &str) -> Option<Arc<K>>;  // key = "namespace/name" or "name"
}
```

### Backend Implementations

| Backend | Type | Data source | Injection timing |
|---|---|---|---|
| `KubeRawStore<K>` | Wraps `reflector::Store<K>` | Maintained automatically by the reflector, in real-time sync with the K8s API | Injected in `run_with_api` immediately after `reflector::store()` |
| `LocalRawStore<K>` | `RwLock<HashMap<String, Arc<K>>>` | Maintained by FileSystem and Etcd controllers during init/apply/delete and worker processing | Injected by the non-Kubernetes resource controllers |

### ProcessorObj Exposed Interfaces

`ProcessorObj` exposes both views through object-safe, typed-erased getters:

```rust
fn get_raw_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;
fn get_cached_any(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>>;
```

`get_raw_any` takes `Arc<K>` from `raw_store` and coerces it directly to
`Arc<dyn Any + Send + Sync>`. `get_cached_any` clones the processed resource
from `ServerCache` and wraps it in the same typed-erased form. Neither path
serializes the object. The caller restores the concrete type with
`downcast::<ConcreteType>()`:

```rust
let arc: Arc<Gateway> = processor.get_raw_any(&key)?.downcast::<Gateway>().ok()?;
```

When `raw_store` has not been injected (before the init phase), it returns `None`, behaving the same as before — no regression.

Choose the view by the field being consumed:

- Use `get_raw_any` when handler-filtered resources must remain visible, or for
  the current resource inside `on_change` because cache save happens afterward.
- Use `get_cached_any` when the decision must match the object sent through
  conf_sync or must observe Controller-derived fields populated by `preparse`
  or `parse` (for example, PluginPolicy stamps or resolved references).
- Never combine a ServerCache existence check with RawStore content. That can
  validate one generation/view while the Gateway consumes another.

Processed objects can contain resolved secrets. Callers must downcast and
project only the required fields, never serialize the returned object directly
to an external response.

### Callers

| Function | Location | Lookup target |
|---|---|---|
| `lookup_gateway` | `route_utils.rs` | `runtime.processor_registry.get("Gateway")?...downcast::<Gateway>()` |
| `lookup_service_cluster_ip` | `gateway.rs` | `runtime.processor_registry.get("Service")?...downcast::<Service>()` |
| `ai_proxy_admission_state` | `http_route.rs` | Processed `EdgionPlugins` with Controller-stamped `policy_action` |

### Key Files

- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/raw_store.rs` — `RawStore<K>` trait, `KubeRawStore<K>`, `LocalRawStore<K>`
