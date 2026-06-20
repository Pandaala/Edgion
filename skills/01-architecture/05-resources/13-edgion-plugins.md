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

1. **Clear stale references**: clear records for this EdgionPlugins in SecretRefManager
2. **BasicAuth Secret resolution**: iterate `secret_refs`, read username/password from each Secret, populate `resolved_users`
3. **JwtAuth Secret resolution**: iterate `secret_refs`, read JWT signing keys from each Secret (supports RSA/EC/HMAC/EdDSA algorithms), parse public-key metadata (KeyMetadata), populate `resolved_credentials`
4. **JweAuth Secret resolution**: similar to JwtAuth, parse JWE encryption keys
5. **HmacAuth Secret resolution**: read HMAC credentials (access_key + secret_key) from each Secret, populate `resolved_credentials`
6. **KeyAuth Secret resolution**: read API keys from each Secret, populate `resolved_keys`
7. **OIDC Secret resolution**: read client_secret and session_secret from the Secret
8. **CertAuth certificate resolution**: based on `cert_source` mode (Inline/Secret/SecretCa), read the CA certificate from a Secret
9. **Every Secret reference** is registered in SecretRefManager so that Secret changes automatically requeue this resource

### on_delete

Clear all references for this EdgionPlugins in SecretRefManager.

### update_status

Two Gateway-API-style conditions (`k8s_openapi::Condition` type):

- **`Accepted`**: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist. Stays `True` even when inline IP lists have bad entries (partial-good; see `PartiallyInvalid` below).
- **`ResolvedRefs`**: checks Secret references and EdgionConfigData cross-namespace refs:
  - Secret refs: aggregate via `SecretRefManager::get_dependencies(ep_key)`, check `secret_store.get(ns, name)` one by one. Any unresolved Secret → `ResolvedRefsError::SecretNotFound` (reason `BackendNotFound`); the message lists all missing `ns/name`.
  - EdgionConfigData cross-ns refs (`overrideRef`, `GlobalAccessControl.activeProfileRef`, `allowRefs`/`denyRefs`): if `ref_denied` is set on the ref (cross-ns without a matching `ReferenceGrant` and target `visibility ≠ Cluster`), → `ResolvedRefsError::RefNotPermitted` (reason `RefNotPermitted`). If the target is missing/disabled/wrong-type → reason `InvalidEdgionConfigDataRef`.
  - When all references resolve → True
- **`PartiallyInvalid`**: set when `IpRestriction` inline `allow`/`deny` groups contain bad entries (invalid CIDR, empty group, cross-group duplicate CIDR, duplicate/invalid group name). `status=True`/`reason=RuleInvalid` when bad entries exist (message lists them); `status=False`/`reason=AllRulesValid` when clean. The plugin still loads — valid entries compile and the plugin accepts requests normally. A skipped inline **deny** entry additionally increments `edgion_ip_deny_entries_skipped_total{scope="http"}` (blacklist hole = loud metric). The exception: if **both** `allow` and `deny` are completely absent or empty and no `allowRefs`/`denyRefs` are configured, `validate_and_init` returns `Err` and the plugin sets `init_failed` (deny-all fail-closed); this is a structural misconfiguration, not a bad-CIDR case.

Note: `ResolvedRefs` currently checks only Secret **existence**, not whether the Secret contains the required keys (e.g., a BasicAuth Secret missing the `username` field). The latter only emits `tracing::warn!` during parse and is not reflected in the condition.

## Gateway-side processing

After EdgionPlugins is synced to the Gateway side, it is stored in plugin-configuration storage. HTTPRoute / GRPCRoute references EdgionPlugins via ExtensionRef; during route preparse, a `PluginRuntime` execution chain is built. During request handling, plugins are executed in the configured execution stages (request_plugins / response_plugins) in order.

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionPlugins → Secret | Secret | secret_refs (multiple plugins) | Authentication plugins reference credentials in Secrets |
| EdgionPlugins → EdgionConfigData | EdgionConfigData | `overrideRef` (RegionRouteConfig → RegionRouteOverride), `GlobalAccessControl.activeProfileRef` (→ Selector), `IpRestriction.allowRefs`/`denyRefs` (→ IpList) | Cross-ns refs gated by ReferenceGrant + target `visibility`; denied cross-ns ref → `ref_denied` → `ResolvedRefs=False/RefNotPermitted` |
| EdgionPlugins ← HTTPRoute | HTTPRoute | ExtensionRef filter | HTTPRoute references the plugin via ExtensionRef |
| EdgionPlugins ← GRPCRoute | GRPCRoute | ExtensionRef filter | GRPCRoute references the plugin via ExtensionRef |
| EdgionPlugins ← SecretRefManager | Secret | cascading requeue | Secret changes automatically re-process EdgionPlugins |
| EdgionPlugins ← EdgionConfigDataRefManager | EdgionConfigData | cascading requeue | EdgionConfigData changes automatically re-process EdgionPlugins (recomputes ref_denied + status) |
