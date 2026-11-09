---
name: resource-reference-contract
description: Shared resource reference contract — the runtime-independent `edgion_resources::resources::refs` module, per-field default profiles (omitted / empty / literal `core` / required kind / namespace), the finite custom convenience manifest, fixed-identity references, and Grant policy boundaries. Read before adding or changing any field that names another resource.
---

# 06 Resource reference contract

## Canon

`edgion-resources/src/resources/refs/` is the single runtime-independent
reference module shared by Resources, Controller and Gateway. Each family of
fields has an **owning field adapter** (`route_fields`, `gateway_tls_fields`,
`backend_tls_fields`, `plugin_fields`, `link_sys_fields`, `egc_tls_fields`,
`dns_fields`, `acme_fields`, ...). The adapter borrows the actual typed owner,
selects the field's default profile and supported targets, and hands out a
sealed `ReferenceToken` / bound loan. Identity validation, `GrantQuery`,
dependency registration, material lookup and Gateway consumption all start
from that same loan.

Rules for new code:

- Never build a reference identity from detached `(namespace, group, kind,
  name)` strings or a copied raw ref. Add or extend an owning adapter instead.
- An unlisted field never inherits a convenience from its wire type. A new
  convenience requires an explicit manifest entry below.
- Identity and exact comparison are shared; authorization stores, resource
  stores and failure policy belong to the consuming runtime.
- Invalid identity performs no lookup and registers no dependency. A valid but
  missing or denied reference keeps its dependency edge so it recovers.
- Grant checks use the actual field's canonical `GrantQuery`
  (`ReferenceGrantStore::check_query`; bound material uses
  `query_authorization_revision` for one snapshot). See
  [ReferenceGrant](../05-resources/11-reference-grant.md).

## Group semantics

- Kubernetes core is the **empty** group. Gateway API defines omitted/empty
  group as core; it does not define the literal string `core` as an alias.
- `is_core_api_group` (`edgion-resources/src/resources/common/mod.rs`) accepts
  `""` and `"core"` and is only for the finite custom convenience fields below.
  Native Gateway API fields never use it.
- Literal `core` on a native field is structurally admitted by the CRD (it is a
  syntactically valid group) and then rejected at runtime: for a backendRef the
  rule reports `ResolvedRefs=False/InvalidKind` and the slot keeps its weighted
  share as a 500 (see
  [weighted invalid backendRefs](../../04-review/routing/regression-guards/weighted-invalid-backendrefs-retain-traffic-share.md)).
- Pseudo backend kinds (`ServiceClusterIp`, `ServiceExternalName`,
  `ServiceEndpointSlice`, `EndpointSlice`) are rejected. An actual
  `Service.spec.type: ExternalName` selects DNS behavior with kind `Service`.
- ExtensionRef always requires exact `edgion.io/EdgionPlugins`; ReferenceGrant
  targets for core objects use group `""`.

## Default profiles

| Profile | Group/kind defaults | Namespace/scope | Literal `core` |
| --- | --- | --- | --- |
| BackendHttp/Grpc/L4 | omitted group empty; omitted kind Service | route namespace when omitted; cross-namespace needs Grant | rejected |
| MirrorCore | omitted group empty; omitted kind Service | route/plugin namespace; delegated origin retained | rejected |
| Parent | omitted Gateway API group/Gateway | referring namespace; section/port retained | no alias |
| ExtensionLocal | required exact `edgion.io/EdgionPlugins` | route-local | no alias |
| StandardSecret | omitted group empty/kind Secret | Gateway namespace or explicit target + Grant | rejected |
| FrontendCa / LocalCa / PolicyTarget / Parameters | required group/kind/name, exact classifier | field-specific (BackendTLSPolicy CA/client is policy-local only) | no alias |
| GrantSource / GrantTarget | required group/kind, exact comparison | from namespace required; optional wildcard to-name | distinct identity |
| StrictCustomSecret (EdgionTls, EdgionAcme) | omitted group empty/kind Secret | owner contract | rejected |
| CustomSecret / CustomCa* / CustomLocal* / CustomCredential / CustomWebhookService / CustomPolicyTarget | omitted group empty; field-specific kind | owning resource namespace, field-specific access | accepted on core branches only |
| Fixed (ConfigData, LinkSys, global plugins, templates, instance bindings, write destinations) | target type fixed by field or parsed grammar | field-specific | no group input |
| Selectors (backend selector, logical slot, delegation `*`) | selector dimensions, not object defaulting | wildcard/exact semantics preserved | no alias |

## Native fields (strict)

HTTP/GRPC/TCP/TLS/UDP Route `backendRefs[]`, RequestMirror `backendRef`
(route filter and EdgionPlugins config), ExtensionRef, `parentRefs[]`,
`allowedRoutes.kinds[]`, Gateway listener `certificateRefs[]`,
`Gateway.spec.tls.backend.clientCertificateRef`,
`Gateway.spec.tls.frontend.{default,perPort[].tls}.validation.caCertificateRefs[]`,
BackendTLSPolicy `targetRefs[]` and `validation.caCertificateRefs[]`,
GatewayClass `parametersRef`, ReferenceGrant `from[]`/`to[]`, EdgionTls
`secretRef`/`clientAuth.caSecretRef`, EdgionAcme `privateKeySecretRef` /
`externalAccountBinding.keySecretRef`. Vendor Gateway API CRDs are unchanged.

## Finite custom convenience manifest

Only these custom fields accept literal `core` as the empty group:

- **Secret**: EdgionBackend `ai.credentialPool.credentials[].secretRef`;
  LinkSys `config.auth.secretRef` (redis, etcd, elasticsearch, otlp), kafka
  `config.sasl.password.secretRef`, OAuth2 credential source
  `provider.clientAuthentication.{active,previous}SecretRef`; EdgionPlugins
  BasicAuth/KeyAuth/HmacAuth `secretGroups[].secretRefs[]`, JwtAuth
  `issuerGroups[].secretRefs[]`, HeaderCertAuth `caSecretRefs[]`, Csrf and
  JweDecrypt `secretRef`, OpenidConnect `clientSecretRef`/`sessionSecretRef`,
  LdapAuth `bindPasswordRef`,
  Wasm `source.fetch.authHeaderSecretRef`/`source.oci.pullSecretRef`,
  CredentialInjector `credentialRef` (Secret branch only), plus SecretMatch and
  HmacMatch condition leaves in their bounded ConditionSet contexts. Source and
  default namespace is the owning EdgionPlugins / LinkSys / EdgionBackend.
- **TLS** (`T.validation.caCertificateRefs[]`, `T.clientCertificateRef`):
  EdgionGatewayConfig `outboundTls` (true Cluster source, explicit target
  namespace, administrative selection — never an invented source namespace);
  EdgionBackendTrafficPolicy `healthCheck.active.tls` (policy-local only,
  cross-namespace rejected regardless of Grants); LinkSys `config.tls`,
  `config.connection.tls` (httpdns), `config.provider.tls` (Secret-only CA,
  existing Grant checks); EdgionPlugins OpenidConnect/LdapAuth `tls`, ExtProc
  `grpcService.tls`, Wasm `source.fetch.tls`, Guardrail `tls`, AiGuard
  `evaluator.tls`, ForwardAuth `tls` (Secret/ConfigMap CA, Grant-gated).
- **Service/policy**: LinkSys webhook `config.target`, ForwardAuth/Guardrail
  `target`, AiGuard `evaluator.target` (omitted namespace = owning resource
  namespace; no Service ReferenceGrant check), EdgionBackendTrafficPolicy
  `targetRefs[]` (core Service branch only; EdgionBackend exact).

## Fixed identities

Fixed-type references keep their own grammar and failure policy; they gain no
group/kind input and no alias. Examples: `Gateway.spec.gatewayClassName`; EGC
`globalPluginsRef[]` (omitted namespace is `default`) and
`dnsResolver.linkSysRef`; DSL `linksys_refs[]` (name is also the DSL alias;
`rateLimitPolicies[].namespace` is a Redis counter namespace); Redis/Webhook
LinkSys strings; log output `output::LinkSysRef`; ConfigData refs (IpList,
RegionRoute override, Canary/WAF/RequestAccess profiles); Gateway/Route plugin
and stream-plugin annotations; `edgion.io/dns-resolver-ref` on Gateway and
Service (actual owner is the Grant source); `${secretRef:ns/name:key}` templates;
dedicated Service `gateway-service-ref` + UID; `EDGION_GATEWAY_SCOPE`/UID;
ACME storage and generated EdgionTls outputs (write destinations, not lookups);
generated credential Secrets; EndpointSlice `kubernetes.io/service-name`;
DNS-generated EndpointSlices (Service-UID owned, forced SSA).

Controller and Gateway keep **different** malformed EndpointSlice fallbacks
(Controller ignores a missing label and defaults namespace to `default`;
Gateway falls back to `slice.name`). Do not harmonize without an impact review.

## Grant policy boundaries

- `enable_reference_grant_validation` is process-wide and default-on; its effect
  is field-specific. Turning it off never relaxes identity syntax or removes
  recovery dependency edges.
- ConfigData `Cluster` visibility needs no matching Grant.
- Cross-namespace Gateway serving certificates, frontend CA ConfigMaps/Secrets,
  backend client certificates and route backends are Grant-gated; revocation
  takes effect on the next generation without restarts.

## Verification evidence

The contract was accepted on 2026-11-05 after unit/workspace gates, Standard
v1.6.2 conformance (HTTP Core 37, Extended 30, GRPC Core 15, TLS Core 20), live
Kubernetes admission and Grant lifecycle traffic checks, and FS-mode dynamic
scenarios. The ACME `AcmeGatewayRefOwnership` dynamic scenario is FS-only; the
Kubernetes exclusion and its re-evaluation trigger live in
`edgion-tests/k8s/scripts/common/k8s_suite_scope_exceptions.tsv`. The etcd
source mode has no live integration harness; its parity rests on the shared
typed ingest path and unit coverage.
