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
  Native Gateway API fields never use it. The same alias is also applied inline
  (not yet through that helper; see the
  [follow-up todo](../../../tasks/todo/resource-reference-contract-follow-ups.md)):
  `OwnedTarget::prepare_in_namespace` for custom Secret/ConfigMap fields in
  `edgion-resources/src/resources/refs/owned.rs`,
  `PolicyTargetRef::canonical_group` in
  `edgion-resources/src/resources/refs/targets.rs`, and the EdgionBackend
  `secret_ref_validation_errors` check in
  `edgion-resources/src/resources/edgion_backend.rs`.
- Literal `core` on a native field is structurally admitted by the CRD (it is a
  syntactically valid group) and then rejected at runtime: for a backendRef the
  rule reports `ResolvedRefs=False/InvalidKind` and the slot keeps its weighted
  share as a 500 (see
  [weighted invalid backendRefs](../../04-review/routing/regression-guards/weighted-invalid-backendrefs-retain-traffic-share.md)).
- Within the core group, native backendRefs accept only exact kind `Service`;
  pseudo backend kinds (`ServiceClusterIp`, `ServiceExternalName`,
  `ServiceEndpointSlice`, `EndpointSlice`) are rejected there. Outside the core
  group, HTTPRoute backendRefs also accept `edgion.io/EdgionBackend` and
  HTTPRoute delegation, and GRPCRoute backendRefs accept GRPCRoute delegation
  (`HTTPBackendRef::validated_target` / `GRPCBackendRef::validated_target` in
  `edgion-resources/src/resources/refs/targets.rs`; see
  [EdgionBackend](../05-resources/22-edgion-backend.md) and
  [HTTPRoute](../05-resources/04-http-route.md)). TCP/TLS/UDP backendRefs and
  `BackendObjectReference` are Service-only. An actual
  `Service.spec.type: ExternalName` selects DNS behavior with kind `Service`.
  Custom managed-HTTP Service targets (LinkSys webhook, ForwardAuth/Guardrail,
  AiGuard) still accept `ServiceClusterIp` and `ServiceEndpointSlice` (also
  spelled `EndpointSlice`) as transport modes of a core Service identity
  (`managed_http_service_target` in
  `edgion-resources/src/resources/refs/targets.rs`; see
  [webhook](../03-controller-gateway-link/webhook.md) and
  [LinkSys schema](../../02-features/03-resources/18-link-sys.md)).
  `ServiceImport` remains unimplemented on both paths.
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
RegionRoute `clusterRegionRef` (ClusterRegionSetting), Canary/WAF/RequestAccess
profiles); GrpcJsonTranscoder
`descriptor` with `type: configMapRef` (fixed core ConfigMap in the owning
EdgionPlugins namespace, exact `binaryData[key]`, no cross-namespace or Grant
capability; see the
[descriptor ownership guard](../../04-review/resources/regression-guards/grpc-descriptor-configmap-ownership.md));
Gateway/Route plugin and stream-plugin annotations; `edgion.io/dns-resolver-ref` on Gateway and
Service (actual owner is the Grant source); `${secretRef:ns/name:key}` templates;
dedicated Service `gateway-service-ref` + UID; `EDGION_GATEWAY_SCOPE`/UID;
ACME storage and generated EdgionTls outputs (write destinations, not lookups);
generated credential Secrets; EndpointSlice `kubernetes.io/service-name`;
DNS-generated EndpointSlices (in-memory synthetic slices, never written to the
apiserver; see the Controller DNS resolver).

Controller and Gateway keep **different** malformed EndpointSlice fallbacks
(Controller ignores a missing label and defaults namespace to `default`;
Gateway falls back to `slice.name` for a missing label and keeps a
namespace-less, unqualified Service key when the slice namespace is absent).
Do not harmonize without an impact review.

## Grant policy boundaries

- `enable_reference_grant_validation` is process-wide and default-on; its effect
  is field-specific. Turning it off never relaxes identity syntax or removes
  recovery dependency edges.
- ConfigData `Cluster` visibility needs no matching Grant.
- Cross-namespace Gateway serving certificates, frontend CA ConfigMaps/Secrets,
  backend client certificates and route backends are Grant-gated; revocation
  takes effect on the next generation without restarts.

## Related pages and exclusions

- EGC `outboundTls` administrative selection authority:
  [EdgionGatewayConfig](../05-resources/03-edgion-gateway-config.md).
- EdgionPlugins missing/unique/ambiguous source contract:
  [reference-target ambiguity](../../04-review/plugin-runtime/intentional-design/edgionplugins-key-scoped-last-known-good.md#edgionplugins-reference-target-ambiguity).
- Admin API namespace admission:
  [URL/body identity](../../04-review/admin-api/regression-guards/update-namespaced-reject-body-name-mismatch.md).
- LinkSys TLS CA fields are Secret-only. A well-formed non-Secret CA kind is
  reported invalid by OTLP and skipped by every other provider
  (`LinkSysCaSlot::UnsupportedSkipped` in
  `edgion-resources/src/resources/refs/link_sys_fields.rs`); neither path
  queries a store for that kind. LinkSys has no ConfigMap CA support.
- Classified exclusions, which are not reference fields under this contract:
  service-owned conf_sync CA/leaf Secrets and the leader-election Lease
  bootstrap/ownership protocols; generic `ObjectMeta.ownerReferences`; unused
  EndpointSlice `endpoints[].targetRef`; Controller etcd bootstrap material
  (TLS reads local PEM files and auth uses inline config credentials, without
  resource references or synthetic Secrets); external OCI references, files,
  DNS/network addresses and request/header/session identifiers; the Gateway
  listener `tls.options` key `edgion.io/cert-provider` (a provider selector,
  not a named object reference); and the EdgionTls metadata annotation
  `edgion.io/expose-client-cert` (a boolean). Namespace label selectors keep
  their own policy and watch path.

## Verification evidence

Each item carries its own revision and mode; none is a single whole-product run.

- Standard v1.6.2 conformance (HTTP Core 37 / Extended 30, GRPC Core 15, TLS
  Core 20) on product `6f566a8ae` (2026-10-11), which predates the weighted
  correction below. See the
  [conformance reports](../../../edgion-tests/conformance/reports/README.md).
- Kubernetes API-server admission (34/34) and targeted runtime / Grant lifecycle
  checks on `5e359f300` (2026-10-27): 22 of 23 runtime checks passed; the
  failing weighted case is the one corrected below. The selected Kubernetes
  dynamic scenarios (`CredentialInjectorDynamic`, `RequestMirrorReferenceGrant`,
  `PmdRefFlap`, `BtlspRefFlap`) passed 4/4 in the same run.
- Weighted invalid-backendRef correction: verified on `d01698e3e` plus the
  scoped working-tree fix, then committed in `b69df5592` together with unrelated
  changes; Kubernetes `PartiallyInvalid` passed 4/4 (see
  [weighted invalid backendRefs](../../04-review/routing/regression-guards/weighted-invalid-backendrefs-retain-traffic-share.md)).
- Source-mode parity on `d4fb7f4ad` (2026-11-12): FS file loader, FS Admin API
  and etcd Admin API, plus API-server-defaulted objects (server-side dry-run)
  replayed through the etcd Admin API. This is not a Kubernetes source-mode run.
  Fixtures covered Gateway listener `certificateRefs` (omitted, explicit, literal
  `core`, cross-namespace with and without a Grant), HTTPRoute backendRefs
  (including a mixed weighted invalid share), ExtensionRef, and the
  CredentialInjector Secret group (omitted, empty, `core`). The defaulted replay
  covered only the Gateway and 11 HTTPRoutes, and the dry-run step itself was
  not retained. Conditions, traffic and TLS outcomes were identical across all
  variants; raw differences were limited to normalization and defaulting plus
  per-process attachment-proof identities.
- Native frontend-CA / backend client-certificate Grant matrix: FS mode only,
  Secret CA only, default frontend validation only (no `perPort`, no ConfigMap
  CA). The first run on `d4fb7f4ad` matched the expected traffic and
  authorization outcomes on inspection in all eight Grant phases, and exposed a
  Gateway-level `ResolvedRefs` status gap for
  `spec.tls.backend.clientCertificateRef`. A rerun on `d4fb7f4ad` plus the then
  uncommitted Gateway status fix showed Gateway `ResolvedRefs=False/RefNotPermitted`
  exactly in the phases without a matching client-certificate Grant (P0, P2, P4,
  P6) and `True` otherwise, with traffic unchanged (see the
  [backend client-certificate status guard](../../04-review/status/regression-guards/gateway-backend-client-certificate-resolved-refs.md)).
  The driver has no assertions; outcomes were checked by inspection.
- `AcmeGatewayRefOwnership` passed in FS mode; its Kubernetes run is a recorded
  harness exception in
  `edgion-tests/k8s/scripts/common/k8s_suite_scope_exceptions.tsv`.

Limits: the 2026-11-12 runs were manual one-off drivers whose evidence stayed
local and is not retained in the repository. etcd has a leader-failover script
(`edgion-tests/integration/scripts/integration/etcd_leader/run_etcd_leader_failover.sh`),
but the resource integration and dynamic suites have no etcd-mode run
(`run_integration.sh` has no etcd mode). No full `cargo test --all` and no
conformance rerun followed the weighted correction. Reference-module changes
after the `5e359f300` Kubernetes run, including the route-field change
`6bd9196fb` (2026-11-05), have no Kubernetes live run; the 2026-11-12 FS/etcd
runs exercise them only where their fixtures apply (for example route fields).
`d4fb7f4ad` itself changed Gateway plugin-attachment failure policy (invalid
attachment slots become fail-closed sentinels); its unit and integration
verification is recorded in
[gw-core-02b](../../04-review/plugin-runtime/regression-guards/scoped-policy-runtime-publication.md#malformed-gateway-level-and-egc-global-references-fail-closed-gw-core-02b),
but no independent review of that change is recorded. The contract task closed
on 2026-11-12. Unmet documentation, permanent dynamic-scenario and
alias-consolidation items are tracked in the
[follow-up todo](../../../tasks/todo/resource-reference-contract-follow-ups.md).

## Resolution references

- `core-api-group-alias-contract` — closed 2026-11-12. The standard versus
  Edgion-extension question is answered by [Group semantics](#group-semantics):
  empty group is Kubernetes core, and literal `core` is a finite Edgion custom
  convenience that native Gateway API fields reject.
