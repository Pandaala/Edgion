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
8. **HeaderCertAuth CA resolution**: active Header mode resolves actual `caSecretRefs` and carries only successful Secret siblings containing `ca.crt`; Connection mode clears and ignores CA material. Gateway compilation matches each carried sibling to a valid declared target before CA parsing and entry-cache reuse
9. **Valid active Secret references** are registered before authorization/material acquisition so changes can requeue this resource; invalid references create no dependency or material read
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

- **`Accepted`**: True (reason=Accepted) when there are no validation_errors; False (reason=Invalid) when errors exist. Referenced IpList entry errors belong to that target; removed inline ACL fields make the consumer invalid.
- **`ResolvedRefs`**: checks credential-material references and EdgionConfigData cross-namespace refs:
  - Secret/ConfigMap material refs: readonly classifiers bind the current actual owning fields, validate identity/activity, authorize and query the exact target kind. Dependency registry strings are recovery indexes, never status authority. Fresh status works without parse; stale or disabled-field edges cannot authorize reads or add errors. Missing material reports `SecretNotFound` or `ConfigMapNotFound`. CredentialInjector reports its own missing/denied errors, and final exact deduplication emits one diagnostic for repeated fields. Classifier/field order determines stable messages; the shared priority helper selects the reason. Readonly classification does not register dependencies.
  - Enabled RequestMirror refs: require a core Service with a valid name and port. A missing target sets `ResolvedRefs=False/BackendNotFound`; cross-namespace authorization still uses ReferenceGrant and `RefNotPermitted`.
  - JWE credential content: a present Secret missing `data.secret`, containing invalid inner Base64 for `base64Secret`, or decoding to a non-32-byte A256GCM key sets `ResolvedRefs=False` with `MissingKeys` or `DecodeError`. The Gateway still publishes a fail-closed JWE runtime for that generation and never retains the prior key.
  - EdgionConfigData cross-ns refs (`overrideRef`, `RequestAccessPolicy.activeProfileRef`, WAF policyRef (→ WafPolicy) and plugin activeProfileRef (→ Selector), policy bundle references, RequestAccessPolicy nested IpList refs, `allowRefs`/`denyRefs`): if `ref_denied` is set on the ref (cross-ns without a matching `ReferenceGrant` and target `visibility ≠ Cluster`), → `ResolvedRefsError::RefNotPermitted` (reason `RefNotPermitted`). If the target is missing/disabled/wrong-type → reason `InvalidEdgionConfigDataRef`.
  - When all references resolve → True
- **`PartiallyInvalid`**: reports independently skipped optional WAF profile bundle references. IpRestriction no longer owns CIDR entries; central IpList reports its own partial-good diagnostics. Required WAF bundle failures remain whole-profile failures.

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


## Readonly material-status verification boundary

R4b focused Controller verification passed 935 tests. The family matrix covers
grouped credentials, individual Secrets, local TLS CA/client refs, managed HTTP
templates, conditions, Wasm, descriptors and CredentialInjector without relying
on registry edges. Test-only observers count canonical authorization entry calls
and Secret/ConfigMap store calls. Recording is implemented in the mirrored test
module through an expression macro; production expands directly to the original
operation. An authorization entry call is not necessarily
an internal ReferenceGrant-store read (same-namespace authorization can bypass
that query). Actual event/RawStore/worker tests cover creation, same-UID material
updates, deletion and recreation. Persist-off LinkSys authorization, memory
binding/expiry and retained-generation behavior remain unchanged. This is
in-process evidence, not live delivery or global source-identity closure; see
[coordinator progress](../../../tasks/pending/resource-reference-contract/07-progress.md)
for final gates, independent reviews and commit status.

### Live referenced IP conditions

All shared IP Condition leaves and RAP primary IP fields carry `ipRefs` only.
The Controller clears resolver-only read proofs on every effective owning node,
then recomputes active slots from exact source/target identities. The Gateway
checks source metadata, root/leaf/slot order and scope consistency before binding
original nodes or reusing an entry. The temporary cold address index cannot escape
construction and stores no payload or matcher. Removed inline keys become a
monotone typed invalid marker, preserving publication/recovery through conf_sync.
Primary RAP `negate` presence is invalid; supplemental Condition negation remains.
