---
name: stream-plugin-store-key-normalization
description: Use when reviewing any consumer of the global StreamPluginStore (Gateway annotation, TCPRoute, TLSRoute, UDPRoute, or a future stream-plugin lookup site); enforces the "bare name expanded with the local namespace" normalization that the store key contract requires.
---

# StreamPluginStore key normalization — all consumers must expand bare names

## Conclusion

The global `StreamPluginStore` is **always** keyed by the fully-qualified
`namespace/name` (see `metadata_conf_handler.rs:34` `format!("{}/{}", ns, name)`
and `StreamPluginStore::get_by_ns_name`). Any consumer that translates a user-facing
`edgion.io/edgion-stream-plugins` annotation value into a store-lookup key MUST
expand a bare name using the enclosing resource's namespace before lookup; passing
the raw value through silently fail-closes when an operator writes the
documented short form.

The documented value type on both Gateway and TCPRoute/TLSRoute/UDPRoute tables is
`name` or `namespace/name` (`skills/02-features/05-annotations/references/annotations-metadata.md`).
"Short names are expanded with the local namespace" is the explicit caveat that must
hold at every consumer.

## The two consumer sites — keep them in sync

| Site | File | Namespace source |
|------|------|------------------|
| L4 route resolver (shared by TCPRoute / TLSRoute / UDPRoute) | `src/core/gateway/routes/common.rs::resolve_stream_plugin_store_key` (takes `&ObjectMeta`) | `metadata.namespace` |
| Gateway listener connection-filter (inline copy) | `src/core/gateway/runtime/server/listener_builder.rs::apply_connection_filter` | `context.gateway_namespace` |

The Gateway listener implements the normalization inline because it receives the value
via `ListenerContext` (a `HashMap` + a separate `Option<String>` namespace), not via an
`ObjectMeta` — calling the shared helper would force an awkward fabricated `ObjectMeta`
shim. The two sites are intentionally a tolerated duplicate; this rule entry exists so
the duplication is not silently forgotten when one side evolves.

Both sites implement the same shape:

```rust
let trimmed = annotation_value.trim();
if trimmed.is_empty() { return None; }
if trimmed.contains('/') {
    trimmed.to_string()
} else {
    let ns = <local namespace>.as_deref().unwrap_or("default");
    format!("{}/{}", ns, trimmed)
}
```

Reference fix: F-004 (`tasks/plugins/plugins-stream-F004-...`), 2026-05-26 —
the Gateway listener path historically passed the value through verbatim, so
bare names silently fail-closed the entire listener while logging a misleading
"until resource is synced" warn.

## What to reject in PR review

- **A new StreamPluginStore consumer that uses `annotation_value.trim()` directly
  as the lookup key.** It must apply the bare-name expansion.
- **A "fix" that removes the expansion from either consumer site** on the
  grounds that "operators should always write the fully-qualified form". The
  documented contract is `name OR namespace/name`; tightening it is a behavior
  break on doc-sanctioned configs.
- **Reformulating the fix as a doc change** ("just tell operators to use
  namespace/name"). The store-key contract belongs in code, not in operator
  discipline — the three Route paths already encode it; the Gateway path must
  match.

## Re-evaluation triggers

Re-open this rule only when:

- The `StreamPluginStore` key convention itself changes (e.g. moves to a
  structured `(ns, name)` tuple key instead of a `String`).
- A new resource type starts consuming `edgion.io/edgion-stream-plugins` —
  add a fourth row to the table above and ensure the same normalization runs.
