---
name: stream-plugin-store-key-normalization
description: Use when reviewing stream-plugin attachment normalization; distinguishes the Gateway same-namespace contract from the route name-or-qualified-name contract.
knowledge_type: intentional-design
---

# Stream plugin key normalization depends on the attachment owner

## Conclusion

`PluginPublisher` source and attachment inputs use fully-qualified `namespace/name` strings, but
the operator-facing annotation contract is intentionally different at the two attachment layers:

| Site | Accepted annotation value | Normalization |
|------|---------------------------|---------------|
| `Gateway` listener connection filter | unqualified `name` only | expand with the Gateway namespace |
| `TCPRoute` / `TLSRoute` / `UDPRoute` | `name` or `namespace/name` | expand a bare name with the route namespace; preserve a qualified value |

Gateway-level attachments are same-namespace because the previous qualified-name path loaded an
`EdgionStreamPlugins` from another namespace without `ReferenceGrant` authorization. The Controller
now reports a slash-containing Gateway value as unresolved and does not register the dependency.
`GatewayInput` independently maps such a value to an impossible publication key, preserving
fail-close if configuration bypasses Controller validation.

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
- A new stream attachment consumer must document which namespace owns the attachment and normalize
  to the publisher's `namespace/name` key accordingly. `StreamPluginStore` is now a conf-sync source
  adapter, not a data-plane lookup boundary.

## History

F-004 (2026-05-26) fixed bare-name expansion at the Gateway listener. The later
`gateway-stream-plugins-same-namespace-reference` finding retained that expansion but removed the
unrestricted qualified-name branch.

## Actual stream attachment owner boundary (C2)

The finite `StreamPluginAttachment` facade validates a configured actual owner before
projecting its key. Omitted owner namespace defaults to `default`; explicit empty or
invalid namespace and missing/invalid owner name cannot yield a key. Absent/blank
annotations remain absent even on malformed owners. Preserve raw dependency owner
metadata and existing Gateway clear-before-add/recovery behavior.

Target syntax remains the existing lexical contract: Gateway rejects any slash;
L4 preserves slash-containing trimmed values verbatim and prefixes bare values with
the effective route namespace. These projected keys are not typed target ObjectIdentity
values; this migration does not add target DNS-label restrictions or L4 Grant policy.

Configured invalid fields retain the existing empty-key runtime sentinel, distinct
from absence. Publisher cold demand excludes it before source/ID reads, and Gateway
connection publication returns Unavailable before raw lookup. Even an executable
empty-key raw inventory entry cannot satisfy it. Existing TCP/UDP and TLS missing-view
denial, stage selection, policy handling and captured-view lifetimes remain unchanged.

RouteInput retains the configured-invalid sentinel, but all three route-unit constructors already reject malformed owners through their backend-reference identity gate before stream projection. An absent annotation does not bypass that independent gate.
