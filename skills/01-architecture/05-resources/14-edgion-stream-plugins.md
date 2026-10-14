---
name: resource-edgion-stream-plugins
description: EdgionStreamPlugins resource — Stream/TCP-layer plugin configuration, connection-level filters.
---

# EdgionStreamPlugins resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionStreamPlugins is Edgion's custom-extension resource that defines Stream/TCP-layer plugin configuration. Unlike EdgionPlugins, which works at the HTTP layer, EdgionStreamPlugins runs during connection establishment (ConnectionFilter) and applies to scenarios such as IP restriction and TLS route selection.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs`
- Type definitions: `edgion-resources/src/resources/edgion_stream_plugins/`

## Controller-side processing

### preparse

Shared stateless `PluginValidation` checks enabled current-schema policies.
Unknown connection IpRestriction keys, including removed HTTP-only
`ipSource`/`message`/`status`, receive canonical raw-ingress advisory diagnostics;
they are dropped from typed configuration and never change `Accepted` by themselves.
Removed inline `allow`/`deny` remains structural invalidity even alongside valid
references. Unsupported plugin types are rejected even when disabled.

### parse

- **Cross-PM ref registration**: first `clear_refs_from` clears this resource's old refs in `edgion_config_data_refs`, then observe actual Condition/RealIp/ACL/Geo/Selector sites without clearing between passes, registering each target namespace/name as the cluster_pm_key with value `"EdgionStreamPlugins:<this_ns>/<this_name>"`
- This way, when the referenced GCIR PM changes, the PM handler's `requeue_pm_referrers` will requeue this ESP and refresh ResolvedRefs

### on_delete

- `clear_refs_from` clears all registrations of this resource in `edgion_config_data_refs`

### update_status

Uses `k8s_openapi::Condition`, writing three conditions:

- **`Accepted`**: True (reason=Accepted) when there are no validation errors;
  `Invalid` for current-schema errors. Referenced IpList validation is separate; it stays `True` when referenced IP lists
  have bad entries (partial-good; see `PartiallyInvalid` below).
- **`ResolvedRefs`**: iterates all EdgionConfigData references:
  - `GlobalConnectionIpRestriction.activeProfileRef` (→ Selector): must exist as a `Selector` EdgionConfigData. If `ref_denied` is set (cross-ns without grant / target `visibility: Namespace` without a `ReferenceGrant`): reason `RefNotPermitted`. If missing or wrong-type: reason `InvalidEdgionConfigDataRef`.
  - `IpRestriction.allowRefs`/`denyRefs` (Stage 1 + Stage 2) and GCIR per-profile `ProfileRules.allowRefs`/`denyRefs` (→ IpList): each named `IpList` EdgionConfigData must exist and be enabled. Denied cross-ns ref → reason `RefNotPermitted`; missing/disabled/wrong-type → reason `InvalidEdgionConfigDataRef`.
  - Stage 2 `tls_route_plugins` `IpRestriction` entries are checked here as well.
- **`PartiallyInvalid`**: old consumer-side CIDR diagnostics are cleared. Central IpList owns entry-level partial-good validation; Stream/TLS/GCIR policies contain only references. Removed inline fields are structural invalid input and publish an always-deny generation.

## Gateway-side processing

The source store forwards EdgionStreamPlugins to `PluginPublisher`.
`StreamPluginGeneration` shares demanded entry instances across effective
GatewayClass views. Controller validation remains policy-independent; class
policy from `EdgionGatewayConfig.spec.pluginPolicy` masks `stream/` and
`tlsRoute/` entries at publication. Block denies the applicable new admission;
Bypass excludes the entry before construction.

Listener connection views compare all serving Gateway candidates before routing.
Equivalent views run once; incompatible views deny new connections. Matched
TCP/UDP/TLS routes use the owning Gateway's published view. Established sessions
retain their admitted runtime, and policy edits do not rebuild listeners.
`tlsRoutePlugins` remains a separate stage; no Stage-1 fallback is introduced.
See [scoped publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionStreamPlugins ← Route resources | HTTPRoute/GRPCRoute/TCPRoute/TLSRoute | association reference | Route resources may reference Stream plugins |
| EdgionStreamPlugins → EdgionConfigData | EdgionConfigData | `GlobalConnectionIpRestriction.activeProfileRef` (→ Selector) | Cross-ns refs gated by ReferenceGrant + target `visibility`; denied ref → `ref_denied` → `ResolvedRefs=False/RefNotPermitted` |
| EdgionStreamPlugins → EdgionConfigData | EdgionConfigData | `IpRestriction.allowRefs`/`denyRefs` (Stage 1 + Stage 2); GCIR per-profile `ProfileRules.allowRefs`/`denyRefs` (→ IpList) | Cross-ns refs gated by ReferenceGrant + target `visibility`; denied cross-ns ref → `RefNotPermitted`; missing/disabled/wrong-type → `InvalidEdgionConfigDataRef` |

## Reference-only ACL ownership

Connection IpRestriction, TLS-route IpRestriction and GCIR ProfileRules own ordered allowRefs/denyRefs and scalar policy. IpList alone owns IpGroups and CIDR compilation. Cold constructors require actual field loans and validated side/domain/source/target/slot proofs before cache reuse. Runtime callbacks borrow authorized matchers from one scoped snapshot; GCIR Selector selection uses that same view. Missing optional ACL sources are omitted; nonempty authored allowRefs retain whitelist intent and good siblings remain usable.

Controller passive diagnostics and recovery edges include disabled standalone ACL fields, preserving status and requeue behavior. All imported executable proof/negative metadata is cleared every generation; only valid executable sites receive positive read proof. Passive observation grants no Gateway execution permission. Registry clearing happens once before all reference families are added; apply/delete use the existing serial queue.
