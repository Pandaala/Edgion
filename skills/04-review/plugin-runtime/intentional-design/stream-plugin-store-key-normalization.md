---
name: stream-plugin-store-key-normalization
description: Use when reviewing a consumer of the global StreamPluginStore; distinguishes the Gateway same-namespace attachment contract from the route name-or-qualified-name contract.
knowledge_type: intentional-design
---

# StreamPluginStore key normalization depends on the attachment owner

## Conclusion

The global `StreamPluginStore` is keyed by fully-qualified `namespace/name` strings, but the
operator-facing annotation contract is intentionally different at the two attachment layers:

| Site | Accepted annotation value | Normalization |
|------|---------------------------|---------------|
| `Gateway` listener connection filter | unqualified `name` only | expand with the Gateway namespace |
| `TCPRoute` / `TLSRoute` / `UDPRoute` | `name` or `namespace/name` | expand a bare name with the route namespace; preserve a qualified value |

Gateway-level attachments are same-namespace because the previous qualified-name path loaded an
`EdgionStreamPlugins` from another namespace without `ReferenceGrant` authorization. The Controller
now reports a slash-containing Gateway value as unresolved and does not register the dependency.
The Gateway independently maps such a value to an impossible store key, preserving fail-close if
configuration bypasses Controller validation.

Route-level semantics are outside that decision and must not be tightened as a side effect. Their
shared resolver remains `edgion-gateway/src/routes/common.rs::resolve_stream_plugin_store_key`.

## Reuse without sharing the executable policy

Separate same-namespace `EdgionStreamPlugins` resources may still reuse central lists and selectors
through `EdgionConfigData`. Cross-namespace `EdgionStreamPlugins` → `EdgionConfigData` references
already have explicit `ReferenceGrant` and target `visibility` enforcement. This separates local
ownership of plugin composition and failure behavior from shared ownership of policy data.

## Review rules

- Do not restore qualified Gateway references without a complete design that transports an explicit
  authorization result to the Gateway and retains independent fail-close enforcement.
- Do not apply the Gateway restriction to TCPRoute, TLSRoute, or UDPRoute.
- A new `StreamPluginStore` consumer must document which namespace owns the attachment and normalize
  to the store's `namespace/name` key accordingly.

## History

F-004 (2026-05-26) fixed bare-name expansion at the Gateway listener. The later
`gateway-stream-plugins-same-namespace-reference` finding retained that expansion but removed the
unrestricted qualified-name branch.
