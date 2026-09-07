---
name: gcir-stream-plugin-store-key-precompute
description: Use when reviewing findings that flag `GlobalConnectionIpRestrictionPlugin::on_connection` for a per-connection `format!("{}/{}", ns, name)` allocation inside the PM store getter; the fix is to precompute the store key once at plugin construction and call the existing `_by_key` getter on the hot path.
knowledge_type: regression-guards
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `GlobalConnectionIpRestrictionPlugin` Per-Connection Store-Key `format!` — FIXED (F-008)

## Conclusion

**Confirmed-real, now fixed (F-008, 2026-05-27).** `GlobalConnectionIpRestrictionPlugin::on_connection`
in `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs` runs on the L4
listener accept path (one call per inbound TCP connection). It used to call the by-(ns, name)
getter `EdgionConfigDataStore::get_global_connection_ip_restriction(ns, name)`, which internally
executed `format!("{}/{}", namespace, name)` to build the `HashMap` lookup key, producing one
short-String allocation per connection. The effective namespace resolution
(`ref.namespace.unwrap_or(parent_namespace)`) is a pure constructor-time decision; the lookup
key is therefore fully static per plugin instance.

The fix stores the precomputed key in the plugin and calls the existing key-based getter:

```rust
pub struct GlobalConnectionIpRestrictionPlugin {
    metadata_key: String, // "<effective_ns>/<name>" — built once in new()
}

impl GlobalConnectionIpRestrictionPlugin {
    pub fn new(edgion_config_data_ref: PluginReference, parent_namespace: String) -> Self {
        let effective_ns = edgion_config_data_ref
            .namespace
            .as_deref()
            .unwrap_or(&parent_namespace);
        let metadata_key = format!("{}/{}", effective_ns, edgion_config_data_ref.name);
        Self { metadata_key }
    }
}

// hot path:
let Some(pm) = store.get_global_connection_ip_restriction_by_key(&self.metadata_key) else { ... };
```

`get_global_connection_ip_restriction_by_key(&str)` already exists on
`EdgionConfigDataStore` (`edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs`) and
performs the same `HashMap::get` against the same map; the by-(ns, name) variant is retained
for non-hot-path callers that don't want to build the key themselves.

Re-reports of the form "GlobalConnectionIpRestrictionPlugin allocates a `format!` key per
connection" or "stream IP-restriction plugin should precompute its PM store key" are **not
accepted**; verify the current `plugin.rs` carries `metadata_key: String` + a `_by_key` lookup
and has no per-call `ref_namespace()` / `format!` on the on_connection path before re-reporting.

## Core Rationale

**1. The lookup key is a constructor-time constant.**

`PluginReference::namespace` and `PluginReference::name` are immutable after construction; the
parent namespace passed by the runtime builder is likewise fixed for the plugin's lifetime. The
namespace fallback (`ref.namespace > parent`) collapses to a single `&str` at construction. No
runtime input enters the key derivation, so there is no correctness reason to recompute it per
call.

**2. The accept path is the L4 hot path.**

`on_connection` runs once per inbound TCP connection at the listener gate, before TLS / route
selection. Even a small short-String allocation here multiplies by total connection rate; in
deployments with a high churn of short-lived TCP connections (mTLS handshakes, gRPC unary,
TCP-only L4 proxies) the saved allocation is meaningful versus the work the rest of the gate
does (one map lookup, one IP check). The fix moves a per-connection allocation to a per-plugin
allocation — bounded by plugin-instance count, which only changes on config reconcile.

**3. The `_by_key` getter is the intended hot-path API.**

`EdgionConfigDataStore` already exposes `get_*_by_key(&str)` variants alongside the
`(namespace, name)` convenience methods for exactly this case: callers that have a
precomputed key skip the `format!`. Adopting it preserves the existing dispatch shape with
zero new abstractions.

## Re-evaluation Triggers

Re-open only if:

- The PM store's keying scheme changes (e.g. from `"ns/name"` to a tuple key or a hashed
  representation) and the precomputed `String` field becomes the wrong type. In that case
  precompute the new key type in the same place.
- A new field is added to `PluginReference` that affects effective-key derivation at runtime
  (e.g. a request-time profile selector) — in which case the constructor-time precompute is no
  longer valid for the changed field. The current code is unaffected.

## Reference Cases

- F-008 (BadLogic/Performance, Low), fixed 2026-05-27.
- Source: `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs`.
- Getter: `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs::get_global_connection_ip_restriction_by_key`.

## Resolution references

- `plugins-stream-F008` — 2026-05-27.
