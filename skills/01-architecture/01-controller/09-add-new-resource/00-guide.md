---
name: add-new-resource-guide
description: Workflow for adding a new resource type to Edgion. Use when introducing a new Gateway API, Edgion, or Kubernetes resource and wiring it through the resource type, ResourceKind/defs, controller handlers, config sync, gateway handlers, admin APIs, CRDs, and tests.
---

# Add New Resource

Use this guide when a task adds a brand new resource kind, not when you are only extending an existing spec.

## Start With The Right Mental Model

Current Edgion architecture is:

- Controller side: `ResourceProcessor<T>` owns `ServerCache<T>`
- `ConfigSyncServer` only serves watch/list for processors already registered in `PROCESSOR_REGISTRY`
- Gateway side: `ConfigClient` owns per-kind `ClientCache<T>` and optional `ConfHandler<T>`

Do **not** follow the old pattern of manually adding controller-side `ConfigServer` fields. That is outdated in this repository.

## First Classify The Resource

Before editing files, decide these five things:

1. Is it namespaced or cluster-scoped?
2. Is it a Gateway API resource, an Edgion CRD, or a native K8s resource?
3. Should it sync to Gateway, or remain controller-only?
4. Does it need a controller `ProcessorHandler`, a gateway `ConfHandler`, or both?
5. Does it depend on other resources such as Gateway, Service, Secret, or ReferenceGrant?

These answers determine almost every file you need to touch.

## Recommended Working Pattern

Pick the closest existing resource and trace it end to end before editing.

Useful templates:

- Route-like resources: `HTTPRoute`, `GRPCRoute`, `TLSRoute`
- TLS / Secret dependent resources: `EdgionTls`, `EdgionAcme`
- Plugin-like resources: `EdgionPlugins`, `EdgionStreamPlugins`, `PluginMetaData`
- Controller-only / no-sync resources: `ReferenceGrant`, `Secret`
- Cluster-scoped base config resources: `GatewayClass`, `EdgionGatewayConfig`

Pattern references:

- For a route-like resource that syncs to Gateway and becomes runtime route state, see [references/add-resource-route-like.md](references/add-resource-route-like.md)
- For a controller-only resource that drives validation/requeue but should not sync to Gateway, see [references/add-resource-controller-only.md](references/add-resource-controller-only.md)
- For a plugin-like resource that resolves secrets and becomes reusable runtime config, see [references/add-resource-plugin-like.md](references/add-resource-plugin-like.md)
- For a cluster-scoped base-conf resource, see [references/add-resource-cluster-scoped.md](references/add-resource-cluster-scoped.md)

Useful search pattern:

```bash
rg -n "EdgionAcme|ResourceKind::EdgionAcme|edgion_acme" src config/crd
```

Replace `EdgionAcme` with the closest analog for your new resource.

## Core Checklist

### 1. Define The Resource Type

Always update:

- `src/types/resources/<resource>.rs` or `src/types/resources/<resource>/mod.rs`
- `src/types/resources/mod.rs`
- `src/types/mod.rs` only if you introduce a new export pattern beyond the normal `resources::*` flow

Notes:

- Keep `kind`, `group`, `version`, scope, and status type aligned with the actual API contract.
- If the resource has runtime-only derived fields, follow the existing pattern of `#[serde(skip)]` or dedicated preparse fields used by similar resources.

### 2. Register It In The Resource System

Always update:

- `src/types/resource/kind.rs`
- `src/types/resource/defs.rs`
- `src/types/resource/meta/impls.rs`

Notes:

- `kind.rs` is still manual in this repo. Add the enum variant, `as_str()`, and `from_kind_name()` support.
- `defs.rs` is the single source of truth for cache field names, capacities, aliases, and registry behavior.
- `meta/impls.rs` is where `impl_resource_meta!` lives, including any `pre_parse` hook.
- If the resource should stay controller-only by default, consider whether `DEFAULT_NO_SYNC_KINDS` in `src/types/resource/defs.rs` should include it.

### 3. Wire The Controller Side

Usually update:

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/<resource>.rs`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs`
- `src/core/controller/conf_mgr/conf_center/file_system/controller.rs`
- `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs`

What the handler usually owns:

- validation
- parse / derived runtime fields
- status updates
- dependency registration and requeue behavior

Important rule:

- If `parse()` looks up Gateway relationships, register with `gateway_route_index` in `on_change()` / `on_delete()`
- If it tracks Service refs, Secret refs, or cross-namespace refs, follow the matching helper pattern used by similar handlers
- If the resource needs no special logic, still add a minimal `ProcessorHandler<T>` so it participates in the normal processor lifecycle

### 4. Decide Whether It Syncs To Gateway

If the resource must be available on Gateway, update:

- `src/core/gateway/conf_sync/conf_client/config_client.rs`

Usually that means:

- add a `ClientCache<T>` field
- initialize it in `ConfigClient::new()`
- register a `ConfHandler<T>` if Gateway runtime needs derived state
- wire `get_dyn_cache()`
- wire `list()`
- wire `apply_resource_change()`
- add any direct accessor helpers if the runtime needs them

If the resource is controller-only, do not add Gateway cache wiring.

### 5. Add Gateway Runtime Handling If Needed

If Gateway must turn the synced resource into runtime state, add or update a handler under the relevant subsystem, for example:

- `src/core/gateway/routes/*/conf_handler_impl.rs`
- `src/core/gateway/tls/store/conf_handler.rs`
- `src/core/gateway/link_sys/runtime/conf_handler.rs`
- `src/core/gateway/services/acme/conf_handler_impl.rs`

If the resource is only stored and queried but has no runtime side effects, a dedicated `ConfHandler` may be unnecessary.

### 6. Wire Admin APIs Only If You Need Them

Controller Admin API is explicit, not automatic. If the new resource should support create/update via controller admin endpoints, update:

- namespaced resources: `src/core/controller/api/namespaced_handlers.rs`
- cluster-scoped resources: `src/core/controller/api/cluster_handlers.rs`

Gateway Admin API read endpoints are also explicit. If the new resource should be listed or fetched from Gateway admin endpoints, update:

- `src/core/gateway/api/mod.rs`

### 7. Wire K8s Dynamic Storage If Needed

If the resource should support CRUD in Kubernetes mode through the controller's dynamic writer, update:

- `src/core/controller/conf_mgr/conf_center/kubernetes/storage.rs`

This is especially important for custom Edgion CRDs and any new kind that the controller admin API should be able to create or update in K8s mode.

### 8. Add Or Update CRD / API Manifests

For Edgion CRDs, add or update:

- `config/crd/edgion-crd/*.yaml`

For Gateway API resources, prefer the upstream CRD source already tracked under:

- `config/crd/gateway-api/`

Make sure the Rust type definition and manifest agree on:

- group / version / kind
- scope
- schema fields
- status shape

### 9. Add Tests

At minimum, decide which of these you need:

- Rust unit tests near the resource type or handler
- integration tests under `examples/test/`
- controller / gateway admin API coverage if you exposed new endpoints

For route-like or dependency-heavy resources, integration coverage is usually the real safety net.

## Common Failure Modes

- Adding the type in `defs.rs` but forgetting `kind.rs`
- Adding controller processing but forgetting to spawn the processor in one of the two centers
- Adding Gateway sync but forgetting one of `get_dyn_cache()`, `list()`, or `apply_resource_change()`
- Adding a handler that depends on Gateway / Secret / Service without registering the right requeue path
- Updating Rust types without updating CRD YAML
- Exposing the kind in one admin API but not the other one you expected to use

## Validation

Run the smallest checks that prove the new resource is really wired:

```bash
cargo check
cargo test
./examples/test/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>
```

Also verify that your template resource and the new resource have comparable touchpoints:

```bash
rg -n "ResourceKind::<YourKind>|<YourKind>|<your_resource>" src config/crd examples/test
```

## References

Read these only as needed:

- [references/add-resource-route-like.md](references/add-resource-route-like.md)
- [references/add-resource-controller-only.md](references/add-resource-controller-only.md)
- [references/add-resource-plugin-like.md](references/add-resource-plugin-like.md)
- [references/add-resource-cluster-scoped.md](references/add-resource-cluster-scoped.md)
- [../01-architecture/00-common/03-resource-system.md](../01-architecture/00-common/03-resource-system.md)
- [../01-architecture/01-controller/03-config-center/SKILL.md](../01-architecture/01-controller/03-config-center/SKILL.md)
- [../01-architecture/01-controller/06-requeue-mechanism.md](../01-architecture/01-controller/06-requeue-mechanism.md)
- [../05-testing/01-integration-testing.md](../05-testing/01-integration-testing.md)
- [../../docs/zh-CN/dev-guide/add-new-resource-guide.md](../../docs/zh-CN/dev-guide/add-new-resource-guide.md)
- [../../docs/en/dev-guide/add-new-resource-guide.md](../../docs/en/dev-guide/add-new-resource-guide.md)
