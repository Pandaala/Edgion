# Example Pattern: Plugin-Like Resource

Use `EdgionPlugins` as the reference pattern when the new resource:

- is namespaced
- represents reusable runtime configuration referenced by routes or gateway config
- may need secret resolution before runtime use
- syncs to Gateway and becomes a store-backed runtime object
- often relies on preparse or validation before request-time execution

## Touchpoint Map

### 1. Resource Type And Registration

- `src/types/resources/edgion_plugins/mod.rs`
- `src/types/resources/mod.rs`
- `src/types/resource/kind.rs`
- `src/types/resource/defs.rs`
- `src/types/resource/meta/impls.rs`

Why it matters:

- the type carries reusable plugin config, not direct route attachment state
- `impl_resource_meta!` uses preparse so runtime structures and validation errors are ready early

## 2. Controller Processing

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs`
- `src/core/controller/conf_mgr/conf_center/file_system/controller.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs`

What `EdgionPluginsHandler` demonstrates:

- `preparse()` builds plugin runtime and surfaces preparse errors
- `parse()` resolves secret-backed config such as auth credentials
- `SecretRefManager` registration enables cascading requeue when secrets change
- `on_delete()` clears secret references
- `update_status()` reflects merged validation and preparse results

If your new resource is reusable runtime config with secret-backed fields, this is the main pattern to copy.

## 3. Controller CRUD And K8s Writer

- `src/core/controller/api/namespaced_handlers.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/storage.rs`

Why it matters:

- plugin-like resources are usually operator-managed, so controller-side CRUD is still expected
- custom CRDs need explicit K8s dynamic writer mapping

## 4. Gateway Sync And Runtime Store

- `src/core/gateway/conf_sync/conf_client/config_client.rs`
- `src/core/gateway/plugins/http/conf_handler_impl.rs`
- `src/core/gateway/api/mod.rs`

What this pattern shows:

- `ConfigClient` gets a dedicated `ClientCache<EdgionPlugins>`
- Gateway registers a `PluginStore`-backed `ConfHandler`
- the Gateway-side handler preprocesses incoming data again for runtime safety
- Gateway Admin API exposes the resource for inspection

This differs from route-like resources because the object becomes shared runtime configuration rather than a per-listener route table.

## 5. Manifests And Tests

- `config/crd/edgion-crd/edgion_plugins_crd.yaml`
- `examples/test/conf/EdgionPlugins/`

What to copy:

- add focused integration tests for secret resolution, runtime effect, and any special plugin mode
- keep CRD schema aligned with the plugin config variants actually supported by code

## Minimal Plugin-Like Checklist

- Add the type and register it in `kind.rs`, `defs.rs`, `meta/impls.rs`
- Decide whether preparse is required and wire it through `impl_resource_meta!`
- Add `ProcessorHandler<T>` with any secret/config resolution logic
- Register secret refs if runtime data depends on referenced secrets
- Spawn the processor in both controller centers
- Add namespaced CRUD and K8s dynamic writer support if operators manage it directly
- Add `ClientCache<T>` wiring in Gateway
- Add a store-backed `ConfHandler<T>` for runtime consumption
- Add integration tests for both config sync and runtime behavior

## Choose This Pattern When

Choose the plugin-like pattern if the new resource behaves more like reusable runtime config or a shared store entry than like a route, grant, or gateway-level base config.
