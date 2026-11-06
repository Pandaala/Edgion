# res-01: Gateway `edgion.io/edgion-plugins` attaches another namespace's EdgionPlugins without ReferenceGrant

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-resources/src/constants/annotations.rs:145` (`EDGION_PLUGINS`), `:181` (`parse_plugin_ref_list` keeps any `ns/name` token verbatim)
  - `edgion-resources/src/resources/refs/plugin_attachment_fields.rs:40` (`HttpPluginAttachments::Gateway` turns every qualified token into an attachment key; only lexical identity checks)
  - `edgion-resources/src/resources/refs/plugin_attachment_fields.rs:149` (L4 `StreamPluginAttachment::{Tcp,Tls,Udp}` keep qualified `ns/name` verbatim)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway.rs:634` (dependency registration), `:2822` (`validate_gateway_plugin_ref`: existence check only)
  - `edgion-gateway/src/plugins/publication/inputs.rs:43` (`GatewayInput::from_gateway` publishes the keys with no authorization input)

## Summary

A Gateway in namespace A can name `B/some-plugins` in its `edgion.io/edgion-plugins` annotation and every
request on that Gateway then executes namespace B's EdgionPlugins, including the Controller-resolved secret
material carried inside them. No ReferenceGrant, target-side visibility, or other consent from namespace B
is checked anywhere. This is exactly the defect that was already fixed for the Gateway-level
`edgion.io/edgion-stream-plugins` annotation (`gateway-stream-plugins-same-namespace-reference`), but the
HTTP twin was left open. L4 route-level stream annotations have the same unauthenticated cross-namespace
shape.

## Current behavior (evidence)

- `parse_plugin_ref_list(value, default_ns)` expands bare names with the Gateway namespace and preserves any
  token containing `/` unchanged (`annotations.rs:181-198`).
- `HttpPluginAttachments::Gateway(..).entries()` maps each token to `PluginAttachmentKey::checked(namespace,
  name)`, which only validates DNS-label/subdomain syntax via `ObjectIdentity::checked`. It has no source
  namespace parameter and no grant query.
- Controller `GatewayHandler::parse` registers each key in `plugin_ref_mgr` (`gateway.rs:634-640`);
  `validate_gateway_plugin_ref` (`gateway.rs:2822-2850`) only reports `Missing`/`Ambiguous` and early
  RequestId placement. The other references in the same handler (`dns-resolver-ref`, cross-namespace
  Secrets) do go through ReferenceGrant checks, so the omission is specific to this annotation.
- Gateway publication (`inputs.rs:43-50`) publishes the same keys and the publisher binds the source
  EdgionPlugins, so the plugins run for every request on the Gateway before route plugins.
- The resolved material travels with the source EdgionPlugins: e.g. `CredentialInjectorConfig.resolved_credential`
  (`plugin_configs/credential_injector.rs:47-53`), OIDC client/session secrets, Basic/Key/HMAC/JWT
  credential sets, and DSL/RateLimitRedis LinkSys handles that point at B's Redis/webhooks.
- For L4 routes, `StreamPluginAttachment::key` returns `Some(value.to_owned())` for any slash-containing
  TCPRoute/TLSRoute/UDPRoute annotation value (`plugin_attachment_fields.rs:149-155`); neither Controller
  nor Gateway checks a grant.

## Industry comparison

- Gateway API: every cross-namespace object reference (backendRefs, certificateRefs, etc.) requires a
  ReferenceGrant in the target namespace; `ExtensionRef` filters are `LocalObjectReference` (same
  namespace only) precisely so a route cannot borrow another namespace's filter configuration.
- Kong: a namespaced `KongPlugin` can only be attached to resources in its own namespace; cross-namespace
  sharing requires a cluster-scoped `KongClusterPlugin` created by a cluster admin.
- Envoy Gateway: `SecurityPolicy`/`EnvoyExtensionPolicy` target only same-namespace resources; secrets
  referenced across namespaces need ReferenceGrant.

## Impact

- Credential exfiltration: A's Gateway attaches B's `CredentialInjector` and routes the traffic to an
  A-controlled backend, which receives B's injected upstream credential in clear.
- Confused deputy: A drives B's OIDC client (B's `client_secret`), B's Redis LinkSys (rate-limit/DSL writes
  into B's Redis), B's webhook/ForwardAuth endpoints with B's service credentials.
- The documented sharing model ("keep a local wrapper and share data through EdgionConfigData, which
  enforces ReferenceGrant") is silently bypassed for HTTP.

## Suggested direction

Apply the same contract as the fixed stream case, in one place (`HttpPluginAttachments::Gateway`):

1. Recommended: accept only unqualified names (resolve in the Gateway namespace); a slash-containing token
   becomes an invalid entry (Gateway `ResolvedRefs=False`, no dependency edge) and the Gateway publisher maps
   it to an impossible key so the listener fails closed even if the Controller is bypassed. Shared policy
   belongs in cluster-scoped `EdgionGatewayConfig.globalPluginsRef` (admin authority) or in EdgionConfigData.
2. If cross-namespace HTTP plugin sharing is a real requirement, require a ReferenceGrant
   (`from: Gateway/A`, `to: EdgionPlugins`) evaluated by the Controller, transport the authorization result,
   and fail closed on the data plane — the same design the stream guard says is needed before reintroducing
   qualified names.
3. Apply the same decision to TCPRoute/TLSRoute/UDPRoute `edgion.io/edgion-stream-plugins` (route owners are
   the least-privileged role, so the argument is stronger there than for Gateways).

## Notes

- Checked `skills/04-review/security/regression-guards/gateway-stream-plugins-same-namespace.md` and
  `skills/04-review/plugin-runtime/intentional-design/stream-plugin-store-key-normalization.md`. They fix the
  Gateway stream case for the same reason and explicitly scope out route-level stream attachments ("must not
  be tightened as a side effect"); neither page covers the HTTP `edgion.io/edgion-plugins` annotation, and no
  page records a decision that cross-namespace HTTP attachment is intended. The L4 route item is a request to
  revisit that scope decision with the CredentialInjector/LinkSys evidence above.
- `skills/02-features/05-annotations/references/annotations-metadata.md:30` documents the value as
  "comma/space-separated refs" without any permission model.
- Not tracked under `tasks/` (searched `edgion-plugins`, `HttpPluginAttachments`).

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `edgion-resources/src/constants/annotations.rs:181-198` `parse_plugin_ref_list` keeps any slash-containing token verbatim.
- `refs/plugin_attachment_fields.rs:40-66` `HttpPluginAttachments::Gateway` maps every token to `PluginAttachmentKey::checked(ns, name)` with only syntax checks. There is no grant or source-namespace input.
- The stream twin at `plugin_attachment_fields.rs:148-153` returns `QualifiedGatewayName` for a Gateway owner but `Ok(Some(value))` for TCP/TLS/UDP routes. This matches the finding.
- `security/regression-guards/gateway-stream-plugins-same-namespace.md` fixed exactly this for Gateway stream plugins. Its rationale ("existence validation ... did not establish permission") applies equally to the HTTP annotation, and no page accepts cross-namespace HTTP attachment.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1
- `constants/annotations.rs`, `refs/plugin_attachment_fields.rs`, `handlers/gateway.rs` and `plugins/publication/inputs.rs` are unchanged on this branch; cited lines are current.
- No new review page under `skills/04-review/security` or `plugin-runtime` accepts cross-namespace HTTP plugin attachment; the `reference_grant.rs` handler diff is a refactor unrelated to plugin annotations.
