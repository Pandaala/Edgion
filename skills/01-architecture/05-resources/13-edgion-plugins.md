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

GrpcJsonTranscoder descriptor ConfigMap references use the shared owning plugin
reference walker. The field fixes core ConfigMap and the actual plugin namespace
(default only when the owner namespace is omitted), selects exact binaryData[key],
and has no cross-namespace mode or CA conversion. Only enabled request-stage entries
acquire descriptors. All four stage arrays clear prior resolved bytes/errors before
binding; inline, disabled and unsupported-stage entries acquire no object. Malformed
owner/name and invalid key shape create no dependency/read. Valid missing targets
retain recovery dependencies. Readonly status reports invalid identities and missing
objects directly without registering edges; a present object missing the selected
binary key remains an Accepted compile failure, preserving existing status behavior.

### on_delete

Clear all references for this EdgionPlugins in SecretRefManager and ServiceRefManager.

### update_status

Two Gateway-API-style conditions (`k8s_openapi::Condition` type):

- **`Accepted`**: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist. Stays `True` even when inline IP lists have bad entries (partial-good; see `PartiallyInvalid` below).
- **`ResolvedRefs`**: checks credential-material references and EdgionConfigData cross-namespace refs:
  - Secret/ConfigMap material refs: aggregate via `SecretRefManager::get_dependencies(ep_key)`. Secret keys retain `ns/name`; ConfigMap keys use `ConfigMap/ns/name`, so status applies the declared ReferenceGrant kind and queries the matching store without colliding with a same-name Secret. Missing material reports `SecretNotFound` or `ConfigMapNotFound` respectively.
  - Enabled RequestMirror refs: require a core Service with a valid name and port. A missing target sets `ResolvedRefs=False/BackendNotFound`; cross-namespace authorization still uses ReferenceGrant and `RefNotPermitted`.
  - JWE credential content: a present Secret missing `data.secret`, containing invalid inner Base64 for `base64Secret`, or decoding to a non-32-byte A256GCM key sets `ResolvedRefs=False` with `MissingKeys` or `DecodeError`. The Gateway still publishes a fail-closed JWE runtime for that generation and never retains the prior key.
  - EdgionConfigData cross-ns refs (`overrideRef`, `RequestAccessPolicy.activeProfileRef`, WAF policyRef (→ WafPolicy) and plugin activeProfileRef (→ Selector), policy bundle references, RequestAccessPolicy nested IpList refs, `allowRefs`/`denyRefs`): if `ref_denied` is set on the ref (cross-ns without a matching `ReferenceGrant` and target `visibility ≠ Cluster`), → `ResolvedRefsError::RefNotPermitted` (reason `RefNotPermitted`). If the target is missing/disabled/wrong-type → reason `InvalidEdgionConfigDataRef`.
  - When all references resolve → True
- **`PartiallyInvalid`**: reports independently skipped entries, including bad `IpRestriction` inline `allow`/`deny` groups and optional WAF profile bundle references. A required WAF bundle failure remains a whole-profile failure. `status=True`/`reason=RuleInvalid` identifies skipped entries without exposing rule source; `status=False`/`reason=AllRulesValid` means none were skipped. A skipped inline IP **deny** entry additionally increments `edgion_ip_deny_entries_skipped_total{scope="http"}`. An entirely empty IP restriction remains a structural error.

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
| EdgionPlugins → EdgionConfigData | EdgionConfigData | `overrideRef` (RegionRouteConfig → RegionRouteOverride), `RequestAccessPolicy.activeProfileRef` (→ Selector), WAF `policyRef` (→ WafPolicy) and plugin `activeProfileRef` (→ Selector), whose policy profiles reference bundles, RequestAccessPolicy nested IP sets (→ IpList), `IpRestriction.allowRefs`/`denyRefs` (→ IpList) | Cross-ns refs gated by ReferenceGrant + target `visibility`; denied cross-ns ref → `ref_denied` → `ResolvedRefs=False/RefNotPermitted` |
| EdgionPlugins ← HTTPRoute | HTTPRoute | ExtensionRef filter | HTTPRoute references the plugin via ExtensionRef |
| EdgionPlugins ← GRPCRoute | GRPCRoute | ExtensionRef filter | GRPCRoute references the plugin via ExtensionRef |
| EdgionPlugins ← SecretRefManager | Secret | cascading requeue | Secret changes automatically re-process EdgionPlugins |
| EdgionPlugins ← ServiceRefManager | Service / EndpointSlice | cascading requeue | Mirror target availability changes automatically re-process EdgionPlugins |
| EdgionPlugins ← EdgionConfigDataRefManager | EdgionConfigData | cascading requeue | EdgionConfigData changes automatically re-process EdgionPlugins (recomputes ref_denied + status) |

### RequestMirror actual entry ownership (R3b)

`PluginMirrorRefs` borrows the actual EdgionPlugins and original request-entry
ordinal, independently of credential topology. Authored validated MirrorCore
loans alone mint Grant queries. Valid enabled mirrors register Service dependencies
before permission reads; all valid mirror entries, including disabled entries,
refresh cross-namespace repair dependencies and denial metadata. Service status
checks remain enabled-entry-only and preserve missing-Service precedence.

Gateway compilation receives the actual equal-length policy-masked resource.
Disabled entries retain their positions, and the owning mirror field is validated
before entry-cache reuse. Source replacement owns a new generation cache. Effective
targets compile Service/section keys cold and expose no Grant capability; invalid
or denied mirrors cannot consume Session/body/endpoint/writer state. Existing
cross-namespace mirror LKG exclusions and primary-request best-effort behavior
remain unchanged.

### Descriptor runtime ownership and recovery

ConfigMap-backed GrpcJsonTranscoder construction borrows the actual owning request
entry through PreparedPluginReferences. The existing equal-length policy view keeps
source ordinals; malformed source/name/key is rejected before config compilation,
entry-cache reuse or material consumption. The same entry supplies config and loan.
Inline construction remains independent of object references. Shared lexical name
validation also protects direct descriptor_bytes/compile calls without inventing
an owner or claiming that resolved bytes carry provenance.

For unsupported response stages, ConfigMap descriptors receive material-free
limits/name/key validation before existing alias/dye/wrong-stage checks, including
the schema layer's secondary body pass. Inline compiler diagnostics and runtime
body/transport boundaries are unchanged. See the
[descriptor ownership guard](../../04-review/resources/regression-guards/grpc-descriptor-configmap-ownership.md)
for scoped evidence. The initial result PASS and clean gates were superseded by
R3c-F1: a second review found eager descriptor copies before validation. The
corrected candidate requires renewed workspace gates and independent reviews.
Final acceptance, review and commit status are recorded in
[progress07](../../../tasks/pending/resource-reference-contract/07-progress.md).

ConfigMap events preserve the EP's raw dependency owner key, including omitted
namespace, and workers fetch the current raw UID/generation. Namespaced EP events
continue through Route/Gateway workers. HTTP rule ExtensionRef discovery registers
without reading plugin material: Route Accepted/ResolvedRefs do not mirror EP
Accepted. Gateway descriptor compilation owns invalid-material rejection. The
raw-None semantic-target/raw-key mismatch and generic dependency-string status
reads remain R4 work; this batch proves raw-None EP recovery/notification only.

The R3c-F1 cold-copy boundary covers publication source construction, generation
policy views and direct compilation. It binds the actual owner/entry before the
sole ConfigMap descriptor-byte copy operation, preserves all other fields and
positions, and omits invalid/inactive/unsupported material. Policy demand suppresses
copying into masked views; an earlier eligible source generation may already own
valid bytes. Rate-limit dependency checks borrow original source aliases rather
than cloning entries. Opaque raw input Arcs and source-equality serialization stay
input bookkeeping: this is not a claim that invalid bytes are never stored or
copied anywhere in the process. Inline descriptors remain object-independent.
