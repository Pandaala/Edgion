---
name: resource-edgion-plugins
description: EdgionPlugins resource — HTTP plugin configuration, Secret reference resolution, PluginRuntime build, ExtensionRef binding.
---

# EdgionPlugins resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionPlugins is Edgion's custom-extension resource that defines HTTP-layer plugin configuration. It supports 28 built-in plugins (authentication, rate limiting, rewriting, CORS, etc.) and is referenced via the ExtensionRef filter on HTTPRoute / GRPCRoute.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
- Gateway ConfHandler: `edgion-gateway/src/plugins/http/conf_handler_impl.rs`
- Plugin type definitions: `edgion-resources/src/resources/edgion_plugins/`
- Plugin config types: `edgion-resources/src/resources/edgion_plugins/plugin_configs/`

## Controller-side processing

### parse

The EdgionPluginsHandler parse stage performs extensive Secret-reference resolution to fill in credentials for authentication-class plugins:

1. **Clear stale references**: clear records for this EdgionPlugins in SecretRefManager and ServiceRefManager
2. **BasicAuth Secret resolution**: iterate `secret_refs`, read username/password from each Secret, populate `resolved_users`
3. **JwtAuth Secret resolution**: iterate `secret_refs`, read JWT signing keys from each Secret (supports RSA/EC/HMAC/EdDSA algorithms), parse public-key metadata (KeyMetadata), populate `resolved_credentials`
4. **JweAuth Secret resolution**: similar to JwtAuth, parse JWE encryption keys
5. **HMAC Secret resolution**: one resolver consumes the canonical `HmacMatchConfig` for both standalone `HmacAuth` and nested `RequestAccessPolicy.hmacAuth`, reads credentials from each Secret, and populates `resolved_credentials`. Standalone resolution retains only metadata headers named by `upstreamHeaderFields`; policy resolution discards credential metadata because matching must not mutate the request.
6. **KeyAuth Secret resolution**: read API keys from each Secret, populate `resolved_keys`
7. **OIDC Secret resolution**: read client_secret and session_secret from the Secret
8. **CertAuth certificate resolution**: based on `cert_source` mode (Inline/Secret/SecretCa), read the CA certificate from a Secret
9. **Every Secret reference** is registered in SecretRefManager so that Secret changes automatically requeue this resource
10. **Enabled RequestMirror Service references** are registered under mirror-qualified ServiceRefManager keys so Service and EndpointSlice changes requeue this resource without treating the mirror as a forwarding backend-policy ancestor

### on_delete

Clear all references for this EdgionPlugins in SecretRefManager and ServiceRefManager.

### update_status

Two Gateway-API-style conditions (`k8s_openapi::Condition` type):

- **`Accepted`**: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist. Stays `True` even when inline IP lists have bad entries (partial-good; see `PartiallyInvalid` below).
- **`ResolvedRefs`**: checks credential-material references and EdgionConfigData cross-namespace refs:
  - Secret/ConfigMap material refs: aggregate via `SecretRefManager::get_dependencies(ep_key)`. Secret keys retain `ns/name`; ConfigMap keys use `ConfigMap/ns/name`, so status applies the declared ReferenceGrant kind and queries the matching store without colliding with a same-name Secret. Missing material reports `SecretNotFound` or `ConfigMapNotFound` respectively.
  - Enabled RequestMirror refs: require a core Service with a valid name and port. A missing target sets `ResolvedRefs=False/BackendNotFound`; cross-namespace authorization still uses ReferenceGrant and `RefNotPermitted`.
  - JWE credential content: a present Secret missing `data.secret`, containing invalid inner Base64 for `base64Secret`, or decoding to a non-32-byte A256GCM key sets `ResolvedRefs=False` with `MissingKeys` or `DecodeError`. The Gateway still publishes a fail-closed JWE runtime for that generation and never retains the prior key.
  - EdgionConfigData cross-ns refs (`overrideRef`, `RequestAccessPolicy.activeProfileRef`, RequestAccessPolicy nested IpList refs, `allowRefs`/`denyRefs`): if `ref_denied` is set on the ref (cross-ns without a matching `ReferenceGrant` and target `visibility ≠ Cluster`), → `ResolvedRefsError::RefNotPermitted` (reason `RefNotPermitted`). If the target is missing/disabled/wrong-type → reason `InvalidEdgionConfigDataRef`.
  - When all references resolve → True
- **`PartiallyInvalid`**: set when `IpRestriction` inline `allow`/`deny` groups contain bad entries (invalid CIDR, empty group, cross-group duplicate CIDR, duplicate/invalid group name). `status=True`/`reason=RuleInvalid` when bad entries exist (message lists them); `status=False`/`reason=AllRulesValid` when clean. The plugin still loads — valid entries compile and the plugin accepts requests normally. A skipped inline **deny** entry additionally increments `edgion_ip_deny_entries_skipped_total{scope="http"}` (blacklist hole = loud metric). The exception: if **both** `allow` and `deny` are completely absent or empty and no `allowRefs`/`denyRefs` are configured, `validate_and_init` returns `Err` and the plugin sets `init_failed` (deny-all fail-closed); this is a structural misconfiguration, not a bad-CIDR case.

Most legacy plugin Secret resolvers currently report only Secret **existence** through `ResolvedRefs`, not every required data key. JWE and the other explicitly documented typed resolvers are exceptions: they project usable-material failures into `ResolvedRefs` without turning dependency content into an operator-facing `Accepted` error.

## Gateway-side processing

The source store forwards EdgionPlugins to the canonical plugin publisher.
EGC global references, Gateway annotations and route/backend ExtensionRefs
contribute attachment demand. Each immutable source generation owns shared
entry constructors; the publisher masks entries by current GatewayClass policy
and builds effective views. HTTP requests use their captured class scope across
all four stages and delayed backend lookups. Invalid DSL/WAF retention must pass
current policy and mirror guards; it cannot restore excluded entries.

Controller status describes enabled source configuration and resolved references,
not the execution decision of every class sharing this resource. No global
policy-restriction condition or serialized entry action is produced. See
[scoped publication](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md).

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionPlugins → Secret | Secret | secret_refs (multiple plugins) | Authentication plugins reference credentials in Secrets |
| EdgionPlugins → Service | Service | enabled RequestMirror `backendRef` | Missing targets set `ResolvedRefs=False/BackendNotFound`; Service and EndpointSlice changes trigger requeue |
| EdgionPlugins → EdgionConfigData | EdgionConfigData | `overrideRef` (RegionRouteConfig → RegionRouteOverride), `RequestAccessPolicy.activeProfileRef` (→ Selector), RequestAccessPolicy nested IP sets (→ IpList), `IpRestriction.allowRefs`/`denyRefs` (→ IpList) | Cross-ns refs gated by ReferenceGrant + target `visibility`; denied cross-ns ref → `ref_denied` → `ResolvedRefs=False/RefNotPermitted` |
| EdgionPlugins ← HTTPRoute | HTTPRoute | ExtensionRef filter | HTTPRoute references the plugin via ExtensionRef |
| EdgionPlugins ← GRPCRoute | GRPCRoute | ExtensionRef filter | GRPCRoute references the plugin via ExtensionRef |
| EdgionPlugins ← SecretRefManager | Secret | cascading requeue | Secret changes automatically re-process EdgionPlugins |
| EdgionPlugins ← ServiceRefManager | Service / EndpointSlice | cascading requeue | Mirror target availability changes automatically re-process EdgionPlugins |
| EdgionPlugins ← EdgionConfigDataRefManager | EdgionConfigData | cascading requeue | EdgionConfigData changes automatically re-process EdgionPlugins (recomputes ref_denied + status) |
