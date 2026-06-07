---
module: resource-config-model
title: Unknown EdgionPlugin variant rejects the entire EdgionPlugins resource at the Gateway
severity: high
envoy_reference: xDS unknown-fields tolerance / Envoy's per-resource versioned proto with forward-compat unknown-field preservation
---

# Unknown EdgionPlugin variant rejects the entire EdgionPlugins resource at the Gateway

## Envoy's design (the proven baseline)

In xDS, resources carry proto3 payloads. Proto3's "be liberal in what you accept" rule means unknown
fields (new field numbers, or entire new message types) are silently preserved rather than causing
parse failures. When Envoy receives a resource from a newer control plane that contains an extension
or field it does not know, Envoy tolerates it: the resource is accepted, only the unknown portion is
ignored. This allows a rolling upgrade where the control plane is upgraded first and the data planes
follow — the older data plane doesn't crash or reject good resources because of fields it doesn't
recognise.

## Edgion's current design

`EdgionPlugin` is a Rust `enum` with `#[serde(tag = "type", content = "config")]`
(`src/types/resources/edgion_plugins/edgion_plugin.rs:24`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum EdgionPlugin {
    BasicAuth(BasicAuthConfig),
    Cors(CorsConfig),
    // ... ~30 concrete variants
    RegionRoute(RegionRouteConfig),
    // no catch-all / unknown variant
}
```

At the Gateway, the full `EdgionPlugins` resource is received as JSON and deserialized in one shot via
`serde_json::from_value::<T>(event_data.clone())` (`src/core/gateway/conf_sync/cache_client/event_dispatch.rs:323`).
If the JSON contains any `requestPlugins` entry whose `"type"` field names a variant that does not exist
in the compiled enum — for example a plugin introduced in a newer Controller version — serde returns
`Err`, and the entire `EdgionPlugins` resource is skipped with a `tracing::error!` and
`record_config_event_parse_error`. All other plugin entries in that resource, which may be valid and
already in use by running routes, are also lost.

The same failure path applies to the list (relist) path:
`serde_json::from_str(&list_data.data)` at `event_dispatch.rs:95` returns an error and the list
call returns `tonic::Status::internal`, forcing a retry loop — meaning the Gateway cannot complete
initialization until the Controller is downgraded or the Gateway binary is upgraded.

## The problem / gap

This violates forward compatibility. A common deployment sequence is: upgrade Controller first, then
roll Gateways. During the window where the Controller is new and some Gateways are old, if any
`EdgionPlugins` resource references a plugin type that is new (e.g., a new auth method), the old
Gateway deserialization fails for the entire `EdgionPlugins` resource. Routes referencing that
resource via `ExtensionRef` will find no plugin in the `PluginStore` and either fail open or fail
closed depending on per-plugin behavior — neither is correct.

The problem is structurally similar to a Kubernetes controller that crash-loops on any CRD field it
doesn't understand: a strictly typed enum with no unknown-variant escape hatch bakes in the
assumption that Controller and Gateway are always at the same binary version.

## Impact

- Rolling upgrade (Controller before Gateway) is unsafe when new plugin types are introduced.
- A single `EdgionPlugins` resource containing one unknown plugin type takes out all plugin
  configurations in that resource, including ones the old Gateway fully understands.
- If the resource is in the relist path, the Gateway's `EdgionPlugins` cache never becomes ready,
  blocking serving of all traffic that depends on plugins in that namespace until the version
  mismatch is resolved.

## Verification note

Code confirmed. `EdgionPlugin` enum at `src/types/resources/edgion_plugins/edgion_plugin.rs:25` has `#[serde(tag = "type", content = "config")]` with no catch-all variant. The watch path at `event_dispatch.rs:323` processes per-event (`serde_json::from_value::<T>`) so only the single offending `EdgionPlugins` resource is dropped (the loop at line 315 continues for remaining events). However, the list path at `event_dispatch.rs:95` parses `Vec<EdgionPlugins>` atomically — a single bad plugin type in any resource in the array fails the entire list, losing ALL EdgionPlugins resources for that namespace. The severity claim is thus understated for the list path: not just "all plugins in one resource" are lost but all EdgionPlugins across the list batch.

## Suggested direction

Add an `Unknown` (or `Unsupported`) catch-all variant to `EdgionPlugin`:

```rust
/// Forward-compat catch-all: preserves the raw JSON of an unrecognised plugin type
/// so the surrounding EdgionPlugins resource can still be accepted.
Unknown { type_name: String, raw: serde_json::Value },
```

During preparse / validation on the Gateway, any `Unknown` entry should produce a
`preparse_error`-style log and be skipped during execution (bypass, not reject). The surrounding
`EdgionPlugins` resource must still be stored in `PluginStore` so that all _known_ plugin entries
remain operational. This mirrors the proto3 unknown-field model: accept the envelope, ignore what
you don't understand.

On the Controller side, an `Unknown` variant in the Controller's own preparse should produce a
`Condition` reporting partial degradation, rather than silently passing an empty runtime.
