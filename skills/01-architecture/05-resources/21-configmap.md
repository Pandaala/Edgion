---
name: resource-configmap
description: ConfigMap Controller-only runtime store used for CA reference resolution and dependent requeue.
---

# ConfigMap resource

> Common flow: [00-resource-flow.md](00-resource-flow.md)

ConfigMap is consumed by the Controller for references such as
`BackendTLSPolicy.validation.caCertificateRefs` with `kind: ConfigMap`.

`ConfigMapHandler.parse` writes the object into `ControllerRuntime.configmap_store`. Change/delete
events use the shared reference manager to requeue every registered owner; delete also removes the
object from the runtime store.

The Gateway intentionally has no `ClientCache<ConfigMap>` or ConfHandler. ConfigMap is not in the
literal default `no_sync_kinds`, so it can still appear in Controller `supported_kinds` unless an
operator excludes it; the Gateway skips kinds for which it has no cache. Do not add a data-plane
ConfigMap cache just to mirror the advertisement—resolve and embed only the material the Gateway
actually needs.

Key source:
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/configmap.rs`.
