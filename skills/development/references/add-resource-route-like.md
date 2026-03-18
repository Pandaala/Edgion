# Example Pattern: Route-Like Resource

Use `TLSRoute` as the reference pattern when the new resource:

- is namespaced
- attaches to `Gateway` via `parentRefs`
- references backend `Service`s
- may participate in cross-namespace validation
- must sync to Gateway and become runtime route state

## Touchpoint Map

### 1. Resource Type And Registration

- `src/types/resources/tls_route.rs`
- `src/types/resources/mod.rs`
- `src/types/resource/kind.rs`
- `src/types/resource/defs.rs`
- `src/types/resource/meta/impls.rs`

Why it matters:

- `kind.rs` gives the enum variant and string mapping
- `defs.rs` gives cache field names and registry metadata
- `meta/impls.rs` keeps `ResourceMeta` exhaustive with the rest of the resource system

## 2. Controller Processing

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/tls_route.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs`
- `src/core/controller/conf_mgr/conf_center/file_system/controller.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs`

What `TlsRouteHandler` demonstrates:

- validate backend refs
- record cross-namespace refs for later `ReferenceGrant` revalidation
- register backend `Service` refs for cross-resource requeue
- resolve `parentRefs` into `resolved_ports`
- register in `gateway_route_index`
- update attached-route tracking and parent status

If your new resource also looks up `Gateway` inside `parse()`, this is the pattern to copy.

## 3. Controller CRUD And K8s Writer

- `src/core/controller/api/namespaced_handlers.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/storage.rs`

Why it matters:

- namespaced Admin API create/update is explicit
- Kubernetes-mode dynamic CRUD mapping is also explicit

If you forget either one, the type may compile but still be unusable through expected control-plane workflows.

## 4. Gateway Sync And Runtime

- `src/core/gateway/conf_sync/conf_client/config_client.rs`
- `src/core/gateway/routes/tls/conf_handler_impl.rs`
- `src/core/gateway/api/mod.rs`

What this pattern shows:

- `ConfigClient` gets a dedicated `ClientCache<TLSRoute>`
- `create_tls_route_handler()` turns synced objects into runtime route tables
- Gateway Admin API list/get support is explicit

If your new resource must affect request-time behavior, this is the part that distinguishes it from controller-only resources.

## 5. Manifests And Tests

- `config/crd/gateway-api/gateway-api-experimental-v1.4.0.yaml`
- `examples/test/conf/TLSRoute/`

What to copy:

- use upstream Gateway API CRD source if the kind is standard or experimental Gateway API
- add integration tests for parentRef resolution, runtime matching, and cross-resource timing if applicable

## Minimal Route-Like Checklist

- Add the type and register it in `kind.rs`, `defs.rs`, `meta/impls.rs`
- Add `ProcessorHandler<T>`
- Spawn the processor in both FileSystem and Kubernetes centers
- Add K8s writer mapping and controller Admin API support if CRUD is expected
- Add `ClientCache<T>` wiring in Gateway if the resource syncs
- Add `ConfHandler<T>` if Gateway runtime needs derived state
- Add Gateway Admin API support if operators need to inspect it
- Add integration tests for attachment, dependency resolution, and runtime behavior

## Choose This Pattern When

Choose the route-like pattern if the new resource is closer to `TLSRoute`, `TCPRoute`, `HTTPRoute`, or `GRPCRoute` than to `ReferenceGrant` or `Secret`.
