# Example Pattern: Controller-Only / No-Sync Resource

Use `ReferenceGrant` as the reference pattern when the new resource:

- is processed on the controller side only
- exists to support validation, policy, or requeue behavior
- should not be cached on Gateway by default
- still needs normal controller lifecycle handling and often controller-side CRUD

## Touchpoint Map

### 1. Resource Type And Registration

- `src/types/resources/reference_grant.rs`
- `src/types/resources/mod.rs`
- `src/types/resource/kind.rs`
- `src/types/resource/defs.rs`
- `src/types/resource/meta/impls.rs`

Important note:

- controller-only does **not** mean “skip resource registration”
- it still needs full registration in the type system and controller processor pipeline

### 2. Default No-Sync Behavior

- `src/types/resource/defs.rs`
- `src/core/controller/conf_mgr/processor_registry.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/center.rs`

What this pattern shows:

- `ReferenceGrant` is included in `DEFAULT_NO_SYNC_KINDS`
- `PROCESSOR_REGISTRY.all_watch_objs()` filters no-sync kinds before exposing watch objects to `ConfigSyncServer`
- cache readiness logic treats no-sync kinds as optional for Gateway startup

If a new resource should remain controller-only by default, this is the key pattern to follow.

### 3. Controller Processing

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_grant/`
- `src/core/controller/conf_mgr/conf_center/file_system/controller.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs`

What `ReferenceGrantHandler` demonstrates:

- maintain controller-side global state
- dispatch change events to dependent resources
- avoid Gateway runtime wiring when the resource itself is not needed there

This is the right model for policy resources that influence how other resources are validated or requeued.

### 4. Controller CRUD And K8s Writer

- `src/core/controller/api/namespaced_handlers.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/storage.rs`

Important reminder:

- a controller-only resource can still need controller Admin API create/update
- it can still need Kubernetes-mode dynamic CRUD mapping

Do not confuse “not synced to Gateway” with “not manageable by the controller”.

### 5. Gateway-Side Explicit Non-Wiring

- `src/core/gateway/conf_sync/conf_client/config_client.rs`
- `src/core/gateway/conf_sync/conf_client/grpc_client.rs`
- `src/core/gateway/api/mod.rs`

What this pattern shows:

- `get_dyn_cache()` returns `None`
- Gateway list/apply paths skip or reject the kind explicitly
- Gateway Admin API does not expose the resource as normal runtime data

This explicit non-wiring is useful because it prevents hidden waits, accidental exposure, and ambiguity about whether the kind should exist on Gateway.

### 6. Manifests And Tests

- `config/crd/gateway-api/gateway-api-standard-v1.4.0.yaml`
- `examples/test/conf/base/ReferenceGrant_edgion-test_allow-cross-ns.yaml`
- `examples/test/conf/ref-grant-status/`

What to test:

- dependent resources revalidate when the controller-only resource changes
- status updates propagate correctly
- cross-resource timing works after late arrival

## Minimal Controller-Only Checklist

- Add the type and register it in `kind.rs`, `defs.rs`, `meta/impls.rs`
- Add `ProcessorHandler<T>`
- Spawn the processor in both controller centers
- Keep controller CRUD and K8s dynamic storage support if operators need it
- Add the kind to no-sync behavior if it should stay off Gateway by default
- Make Gateway skip the kind explicitly instead of leaving behavior ambiguous
- Add tests that prove dependent resources are revalidated correctly

## Choose This Pattern When

Choose the controller-only pattern if the new resource behaves more like policy, grant, validation, or controller coordination state than like request-time routing/runtime data.
