# Example Pattern: Cluster-Scoped Base-Conf Resource

Use `GatewayClass` as the primary reference pattern, and compare with `EdgionGatewayConfig` when the new resource:

- is cluster-scoped
- belongs to gateway bootstrap or base configuration
- syncs to Gateway and is looked up globally rather than by namespace/name pair
- uses cluster admin API paths instead of namespaced CRUD

## Touchpoint Map

### 1. Resource Type And Registration

- `src/types/resources/gateway_class.rs`
- `src/types/resources/edgion_gateway_config.rs`
- `src/types/resources/mod.rs`
- `src/types/resource/kind.rs`
- `src/types/resource/defs.rs`
- `src/types/resource/meta/impls.rs`

Important note:

- cluster scope must be reflected consistently in both the Rust type and `defs.rs`
- `GatewayClass` and `EdgionGatewayConfig` are also marked as base-conf resources in the resource system

## 2. Controller Processing

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs`
- `src/core/controller/conf_mgr/conf_center/file_system/controller.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs`

What this pattern shows:

- in Kubernetes mode, cluster-scoped resources are spawned with `spawn_cluster`
- `GatewayClassHandler` demonstrates controller-name filtering
- status handling is often simpler than route resources, but these objects are still part of the normal processor lifecycle

## 3. Cluster-Scoped CRUD And K8s Writer

- `src/core/controller/api/cluster_handlers.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/storage.rs`

Why it matters:

- cluster-scoped create/update/delete uses different controller API handlers
- the K8s dynamic writer needs a cluster-scoped `ApiResource` mapping

If you only wire namespaced CRUD paths, operators will not be able to manage the resource correctly.

## 4. Gateway Sync And Base-Conf Runtime Use

- `src/core/gateway/conf_sync/conf_client/config_client.rs`
- `src/core/gateway/conf_sync/conf_client/grpc_client.rs`
- `src/core/gateway/config/gateway_class/conf_handler_impl.rs`
- `src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`
- `src/core/gateway/runtime/server/base.rs`
- `src/core/gateway/api/mod.rs`

What this pattern shows:

- Gateway keeps dedicated caches for cluster-scoped base-conf resources
- `grpc_client.rs` always ensures `GatewayClass`, `Gateway`, and `EdgionGatewayConfig` are watched for readiness
- runtime startup looks up `GatewayClass` first, then its referenced `EdgionGatewayConfig`
- Gateway Admin API exposes both resources via cluster-style lookups

This is different from ordinary cluster-scoped data because these resources are part of the minimum configuration needed for Gateway startup.

## 5. Manifests And Tests

- `config/crd/gateway-api/gateway-api-standard-v1.4.0.yaml`
- `config/crd/edgion-crd/edgion_gateway_config_crd.yaml`
- `examples/test/conf/base/`

What to copy:

- use upstream Gateway API manifest for standard cluster-scoped resources when applicable
- use Edgion CRD manifests for custom cluster-scoped resources
- add tests that prove gateways can resolve and consume the base-conf chain

## Minimal Cluster-Scoped Checklist

- Add the type and register it in `kind.rs`, `defs.rs`, `meta/impls.rs`
- Make sure scope is cluster-scoped in both the Rust type and K8s storage mapping
- Add `ProcessorHandler<T>`
- Spawn it in both controller centers, using the cluster-scoped path in Kubernetes mode
- Add cluster Admin API support in `cluster_handlers.rs`
- Add Gateway `ClientCache<T>` wiring if the resource syncs to Gateway
- Add a Gateway `ConfHandler<T>` if runtime startup or lookup depends on it
- Add Admin/API visibility and tests for the base-conf lookup chain

## Choose This Pattern When

Choose the cluster-scoped pattern if the new resource behaves more like a global control-plane configuration object than a namespaced route, plugin config, or controller-only policy resource.
