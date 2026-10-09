# Reference field manifest

This finite proposed registry supplements [01-spec.md](01-spec.md). Paths are
operator JSON/YAML paths unless explicitly labeled as Rust symbols. None of the
custom core conveniences are enabled by this document. Before implementation,
freeze this registry with exact source versions, CRD constraints, and consumers.
An unlisted field does not inherit a convenience from its wire type.
Implementation authority is now recorded in 00-context.md; proposal wording below
is historical and does not override its approved finite contracts.

### Current-HEAD inventory addition

External HEAD 4dbcc5495 adds EdgionPlugins request-stage GrpcJsonTranscoder
config.descriptor, when type=configMapRef: fixed core ConfigMap in the actual
plugin owner namespace (omitted owner namespace defaults to default), exact
binaryData[key], no data fallback, no cross-namespace or ReferenceGrant capability.
Inline descriptors create no resource edge. Enabled request entries acquire;
disabled or unsupported stages clear material/error without acquisition. The
prepared owning adapter and bound resolver are implemented and scoped-reviewed;
generic dependency status and runtime attachment migration remain unfinished.
services[] names protobuf services, not Kubernetes Service resources.
Its HTTPRoute rule ExtensionRef attachment remains the only permitted attachment.
New route_utils::contains_grpc_transcoder and changed backend_request_scope_violation
belong to the existing ExtensionRef owning traversal audit; preserve desired-generation
lookups and union validation. This adds no upstream ExternalAuth support.

## Contract profiles

Profile names are illustrative private implementation names, not operator
switches or public caller-selected flags. Owning/binding accessors statically
select them.

| Profile | Group and kind defaults | Namespace/scope | Literal core |
| --- | --- | --- | --- |
| BackendHttp/Grpc/L4 | Omitted group empty; omitted kind Service; protocol-specific real supported targets | Declaring route namespace when omitted | Unsupported |
| MirrorCore | Omitted group empty; omitted kind Service; core Service only | Declaring route/plugin namespace; delegated route origin retained | Unsupported |
| Parent | Omitted Gateway API group/Gateway | Referring resource namespace; retain section/port | No alias |
| ExtensionLocal | Required group/kind/name; exact edgion.io/EdgionPlugins | Declaring route local; no operator namespace | No alias |
| StandardSecret | Omitted group empty/kind Secret | Referring Gateway namespace or explicit target namespace | Unsupported |
| LocalCa/FrontendCa/PolicyTarget/Parameters | Required group/kind/name; exact supported target classifier | Field-specific local/cross-namespace/cluster rules | No alias |
| AllowedRouteType | Omitted Gateway API group; required kind | Type declaration | No alias |
| GrantSource/GrantTarget | Required group/kind; exact comparison | Required from namespace; to grant namespace and optional wildcard name | Distinct non-empty identity |
| StrictCustomSecret | Omitted group empty/kind Secret | Existing ACME/EdgionTls owner contract | Unsupported |
| CustomSecret | Omitted group empty/kind Secret | Approved actual owning/binding resource namespace | Proposed core to empty |
| CustomCaSecretOnly/CustomCaSecretOrConfigMap | Omitted group empty; required kind; allowed target set fixed by field | Exact per-prefix namespace/access rules below | Proposed core to empty on supported core branches |
| CustomLocalCa/CustomLocalSecret | CA kind required Secret/ConfigMap; client omitted kind Secret | Policy namespace only; explicit same namespace permitted | Proposed core to empty |
| CustomPolicyTarget | Omitted group empty; required kind | Policy local; exact EdgionBackend branch | Proposed only for core Service branch |
| CustomCredential | Secret defaults as above; exact edgion.io/LinkSys branch | Owning EdgionPlugins namespace | Proposed only for Secret branch |
| CustomWebhookService | Omitted group empty; existing custom omitted/empty kind Service and documented mode grammar | Owning LinkSys/EdgionPlugins namespace | Proposed core to empty |
| FixedConfigData/LinkSys/GlobalPlugin/Template/InstanceBinding/WriteDestination | Target type fixed by actual field or parsed grammar | Field-specific locality/default/explicit rules and operation intent | No group input |
| BackendSelector/LogicalSlot | Selector dimensions, not an object defaulting profile | Preserve wildcard/exact occurrence dimensions | No inferred alias |

Standard pseudo-kind inputs are not registered resource targets. Their old
behavior is subject to the separate routing decision gate in spec section 4.5.
Custom Webhook mode grammar does not authorize pseudo-kind resource reads;
the actual authorized lookup target is core Service.

Grant policy is Controller-owned, separate from these profiles. The existing
enable_reference_grant_validation switch is process-wide and default-on; its
current effect differs between consumers. Freeze those differences rather than
introduce a universal bypass through the shared module. Turning it off never
changes supported identity syntax or removes required dependency edges and
cannot establish standard conformance. Permission and authorization_revision
queries share the same canonical identity and pure exact Grant matcher; existing
credential-manager revision/UID/resourceVersion publication fences remain intact.

## Standard and existing strict custom fields

These fields never select a core convenience profile:

| Exact field | Profile |
| --- | --- |
| HTTPRoute.spec.rules[].backendRefs[] | BackendHttp |
| GRPCRoute.spec.rules[].backendRefs[] | BackendGrpc |
| TCPRoute/TLSRoute/UDPRoute.spec.rules[].backendRefs[] | BackendL4 |
| HTTPRoute.spec.rules[].filters[].requestMirror.backendRef | MirrorCore |
| HTTPRoute.spec.rules[].backendRefs[].filters[].requestMirror.backendRef | MirrorCore |
| HTTPRoute/GRPCRoute.spec.rules[].filters[].extensionRef | ExtensionLocal |
| HTTPRoute/GRPCRoute.spec.rules[].backendRefs[].filters[].extensionRef | ExtensionLocal |
| HTTPRoute/GRPCRoute/TCPRoute/TLSRoute/UDPRoute.spec.parentRefs[] | Parent |
| Gateway.spec.listeners[].allowedRoutes.kinds[] | AllowedRouteType |
| Gateway.spec.listeners[].allowedRoutes.namespaces.from/selector | Namespace-selection policy; preserve Same/All/Selector and Namespace label events; not a named lookup or Grant capability |
| Gateway.spec.listeners[].tls.certificateRefs[] | StandardSecret |
| Gateway.spec.tls.backend.clientCertificateRef | StandardSecret |
| Gateway.spec.tls.frontend.default.validation.caCertificateRefs[] | FrontendCa |
| Gateway.spec.tls.frontend.perPort[].tls.validation.caCertificateRefs[] | FrontendCa |
| BackendTLSPolicy.spec.targetRefs[] | PolicyTarget |
| BackendTLSPolicy.spec.validation.caCertificateRefs[] | LocalCa |
| GatewayClass.spec.parametersRef | Parameters |
| ReferenceGrant.spec.from[] / spec.to[] | GrantSource / GrantTarget |
| Gateway.spec.gatewayClassName | Fixed cluster GatewayClass, required name; no ReferenceGrant |
| EdgionTls.spec.secretRef / spec.clientAuth.caSecretRef | StrictCustomSecret |
| EdgionAcme.spec.privateKeySecretRef / spec.externalAccountBinding.keySecretRef | StrictCustomSecret |
| EdgionTls.spec.parentRefs[] and existing ACME Gateway/auto-TLS parent fields | Parent |

The EdgionPlugins RequestMirror config.backendRef also uses MirrorCore with
the plugin's own source/default namespace. Its custom container does not make
the group convenience automatically available. Unsupported native gRPC mirror,
Gateway infrastructure parameters, ListenerSet, and ExternalAuth remain excluded.

## Proposed custom Secret fields

| Exact field | Source/default namespace | Definition |
| --- | --- | --- |
| EdgionBackend.spec.ai.credentialPool.credentials[].secretRef | EdgionBackend | AiCredential flattens SecretSlot |
| LinkSys.spec.config.auth.secretRef, type redis | LinkSys | RedisAuth.slot |
| LinkSys.spec.config.auth.secretRef, type etcd | LinkSys | EtcdAuth.slot |
| LinkSys.spec.config.auth.secretRef, type elasticsearch, actual auth variants | LinkSys | ElasticsearchAuth slot variants |
| LinkSys.spec.config.auth.secretRef, type otlp | LinkSys | OtlpAuth.slot |
| LinkSys.spec.config.sasl.password.secretRef, type kafka | LinkSys | KafkaSaslConfig.password is not flattened |
| LinkSys.spec.config.provider.clientAuthentication.activeSecretRef, type credentialSource/provider oauth2ClientCredentials | LinkSys | OAuth2 bootstrap client authentication |
| Same OAuth2 prefix, previousSecretRef | LinkSys | Previous bootstrap identity |

WebhookAuth is currently a defined but unwired enum: WebhookServiceConfig and
ExternalHttpConnection do not contain an auth field. Do not advertise or enable
LinkSys webhook config.auth.secretRef from that definition alone. Existing webhook
template references use their explicit template grammar instead.

For plugin rows below, P is the actual supported entry's config in one of these
four stage roots: EdgionPlugins.spec.requestPlugins[],
spec.upstreamResponseFilterPlugins[], spec.upstreamResponseBodyFilterPlugins[],
or spec.upstreamResponsePlugins[]. A variant is adapted only in stages where it
is already implemented. All rows use EdgionPlugins as source/default namespace,
even when attached by a Gateway, delegated route, or global plugin reference.

| Actual plugin variant | P-relative custom Secret field |
| --- | --- |
| BasicAuth | secretGroups[].secretRefs[] |
| KeyAuth | secretGroups[].secretRefs[] |
| JwtAuth | issuerGroups[].secretRefs[] |
| HmacAuth | secretGroups[].secretRefs[]; flat wire HmacMatchConfig composition |
| HeaderCertAuth | caSecretRefs[] |
| Csrf | secretRef |
| JweDecrypt | secretRef |
| OpenidConnect | clientSecretRef / sessionSecretRef |
| Wasm | source.fetch.authHeaderSecretRef / source.oci.pullSecretRef |
| CredentialInjector | credentialRef; CustomCredential, not Secret-only |

Nested conditions are an explicit part of the allowlist. Existing supported
SecretMatch leaves use SecretMatchCondition.secret_ref, and HmacMatch leaves
use HmacMatchConfig.secret_groups[].secret_refs[]. Preserve the bounded
ConditionSet allOf/anyOf grammar and placement-specific condition validation.
Their finite declaration/binding contexts are:

- Actual plugin entry conditions.skip/run at the four stage roots.
- Entry body.conditions.skip/run only where the stage actually has a body gate.
- Entry dye.request[].conditions.skip/run and dye.response[].conditions.skip/run.
- RequestRestrictionConfig.conditions.
- RequestAccessPolicy profiles/requiredRuleGroups/anyOfRules rule config.conditions
  for the implemented DirectPeerIp/RemoteIp/Url variants and URL candidates.
- RequestAccessUrlAllowList ConfigData items[].conditions placeholders, resolved
  only after binding into the referring EdgionPlugins resolved_candidates.

The last row's authored container is ConfigData; its authorization source,
namespace defaults, credential binding, and wakeup owner are EdgionPlugins.
Use a sealed binding context, not a free profile or arbitrary copied raw ref.
IpAccessMatch.ip_refs in the same trees is FixedConfigData, not CustomSecret.

## Proposed custom TLS fields

For each finite prefix T below, expand only T.validation.caCertificateRefs[]
and T.clientCertificateRef. CA kind is required; the accepted CA target set and
namespace/access policy are field-specific. Client targets are Secret with
omitted kind defaulting to Secret, subject to the same field's scope restriction.

TlsConfig flattens OutboundTlsLocal; there is no intermediate outbound key.

| Exact prefix T | Supported CA kinds | Source, namespace and access policy |
| --- | --- | --- |
| EdgionGatewayConfig.spec.outboundTls | Secret/ConfigMap | True Cluster source; required explicit material namespace; approved exact administrative selection, scoped implementation underway under section 4.7 and 00-context |
| EdgionBackendTrafficPolicy.spec.healthCheck.active.tls | Secret/ConfigMap | Policy source; omitted or explicit policy namespace only; cross-namespace rejected before lookup regardless of grants; existing DirectSameNs resolution |
| LinkSys.spec.config.tls, type redis/etcd/elasticsearch/kafka/otlp/webhook | Secret only | LinkSys namespace when omitted; existing cross-namespace Secret ReferenceGrant checks |
| LinkSys.spec.config.connection.tls, type httpdns | Secret only | LinkSys namespace when omitted; existing cross-namespace Secret ReferenceGrant checks |
| LinkSys.spec.config.provider.tls, type credentialSource/provider oauth2ClientCredentials | Secret only | LinkSys namespace when omitted; existing cross-namespace Secret ReferenceGrant checks |
| P.tls, OpenidConnect/LdapAuth | Secret/ConfigMap | EdgionPlugins namespace when omitted; existing GrantGated CA and cross-namespace client Secret checks |
| P.grpcService.tls, ExtProc | Secret/ConfigMap | EdgionPlugins; existing grant-gated material; request-stage-only; grpcService is not flattened |
| P.source.fetch.tls, Wasm | Secret/ConfigMap | EdgionPlugins; existing grant-gated material |
| P.tls, Guardrail | Secret/ConfigMap | EdgionPlugins; existing grant-gated material; flattened connection |
| P.evaluator.tls, AiGuard | Secret/ConfigMap | EdgionPlugins; existing grant-gated material; flattened connection |
| P.tls, ForwardAuth | Secret/ConfigMap | Approved and implemented actual EdgionPlugins source/default, bound grant-gated material and readonly status; flattened connection; full runtime preparation still pending |

LinkSys auth_spec::resolve_tls deliberately uses TlsCertsResolver and validates
CA Secret types, rather than the generic CA helper. OTLP rejects unsupported CA
kinds; other providers currently skip non-Secret kinds. Preserve those error/skip
policies and existing enabled/all-or-none material rules while sharing identity
logic. This proposal does not add LinkSys ConfigMap CA support. Health-check
locality is a hard field boundary, not a grant exemption permitting cross-namespace
access. Shared wire types must not bypass either restriction.

Health checks do not acquire a new independent Service ref: Service probes use
the policy target, and Webhook probes use their existing parent connection.
Do not enable TLS settings in checker variants that do not support them.

ForwardAuth P.tls is a real embedded field consumed by ManagedOutboundClient,
but the current Controller forward_auth resolver only resolves request templates,
not local CA/client material. Its manifest inclusion identifies this wiring gap,
not established functioning resolution. Wiring it is an explicit proposed
correction in the material unit, with source/default/grant/dependency/status
and runtime tests; it cannot inherit alias support from an unwired generic type.

## Proposed custom Service and policy fields

Only these existing Service/URL target fields use CustomWebhookService:

- LinkSys.spec.config.target with type webhook.
- P.target with plugin ForwardAuth or Guardrail.
- P.evaluator.target with plugin AiGuard.

The Service fields are flattened directly into target, with no backendRef key.
Preserve URL/Service mutual exclusion, required Service name/port, existing mode
grammar, egress validation, and source-specific authorization. Template strings
are separate fixed-reference bindings. Existing Webhook Service fields have no
Controller Service ReferenceGrant check; do not advertise one or silently add
it. Their TLS material refs have their own grant policy. The identity adapter
does not decide access policy or widen watched-resource scope.
EdgionBackendTrafficPolicy.spec.targetRefs[]
uses CustomPolicyTarget; kind remains required and EdgionBackend stays exact.

Current LinkSys webhook injects its own namespace before compilation. Inline
ForwardAuth/Guardrail/AiGuard call ManagedOutboundClient::from_parts without
owner namespace, so build_resolved_target currently falls back to default.
The proposed default for those three fields is the owning EdgionPlugins namespace,
consistent with the manifest's source context. This is an explicit behavior
correction, not preserved current semantics. Its implementation gate must cover
compiler/context constructors, all three outbound clients, explicit namespace,
non-default owner namespace, status/validation, and omission fixtures. Freeze
targets on the configuration path; add no request-time owner lookup.

## Fixed identities, strings, and selectors

These consumers reuse identity/key primitives with their existing field rules;
they neither gain group/kind fields nor core aliases. The list uses actual Rust
field symbols where a feature embeds the same type in several existing owners.
The implementation plan freezes each embedding's operator path and consumers.

| Field/symbol | Existing contract to preserve |
| --- | --- |
| Gateway.spec.gatewayClassName | Fixed Gateway API GatewayClass; cluster target; Controller ownership and Gateway scoped-publication/lifecycle consumers |
| EdgionGatewayConfig.spec.globalPluginsRef[] | Fixed EdgionPlugins; omitted namespace specifically default |
| EdgionGatewayConfig.spec.dnsResolver.linkSysRef | Fixed LinkSys; explicit namespace |
| DslConfig.linksys_refs[] | Fixed LinkSys; explicit namespace; name also DSL alias |
| DslConfig.rateLimitPolicies[].link | Registered linksys_refs alias; rateLimitPolicies[].namespace is a Redis counter namespace, not a Kubernetes namespace |
| EdgionBackend.spec.ai.credentialPool.redisRef | Fixed Redis LinkSys string; existing explicit namespace/name grammar and credential-pool failure policy |
| P.tokenQuota.redisRef, request AiProxy | Fixed Redis LinkSys string; explicit namespace/name; existing quota identity/failure policy |
| P.redisRef, RateLimitRedis | Fixed Redis LinkSys string; existing explicit namespace/name validation and provider/failure policy |
| EdgionStreamPlugins.spec.plugins[].config.redisRef, ConnectionRateLimit | Fixed Redis LinkSys string; existing explicit namespace/name grammar, optional Redis dimensions and failure policy |
| P.vars[].webhookRef, WebhookKeyGet | Fixed Webhook LinkSys string; existing explicit namespace/name grammar, provider-kind checks and default extraction behavior |
| crate::output::LinkSysRef in StringOutput::Es/Kafka/Otlp | Fixed LinkSys with required namespace/name and existing DNS validation; preserve sink provider-kind checks; distinct from plugin_configs::LinkSysRef and its DSL alias |
| LogConfig.output/fallback using those StringOutput variants | Actual logging bindings, including Gateway access_log/ssl_log/tcp_log/tls_log/udp_log; no new ReferenceGrant policy or log ownership/lifecycle changes |
| IpRestrictionConfig.allow_refs/deny_refs | Fixed ConfigData; existing allow/deny failure asymmetry |
| RegionRouteConfig.override_ref | Fixed ConfigData; fail-to-base overlay |
| CanaryConfig.active_profile_ref | Fixed ConfigData; existing profile behavior |
| EdgionPlugins.spec.waf.policyRef | Fixed edgion.io/EdgionConfigData WafPolicy; actual EP source and EP-local/default namespace; EP Grant/visibility; invalid identity has zero access/edges, unresolved candidate preserves existing Gateway LKG eligibility |
| EdgionPlugins.spec.waf.activeProfileRef | Fixed edgion.io/EdgionConfigData Selector; actual EP source independently of policy namespace; EP-local/default target; EP Grant/visibility; invalid/unavailable Selector follows validated default fallback and readonly diagnostics |
| EdgionConfigData.spec.data WafPolicy.profiles[*].bundleRefs[*].target | Fixed edgion.io/EdgionConfigData WafRuleBundle; actual WafPolicy ConfigData source and policy-local/default target; ConfigData Grant/visibility; required profile atomicity, optional partial-good units, digest/pinning and selected-generation LKG; raw owner keys unchanged |
| RequestAccessPolicyConfig.active_profile_ref, UrlAccessRuleConfig.config_refs[].reference | Fixed ConfigData; bounded binding and visibility |
| GlobalConnectionIpRestrictionConfig.active_profile_ref/allow_refs/deny_refs | Fixed ConfigData; exact owning context and visibility |
| ConnectionIpRestrictionRules.allow_refs/deny_refs, IpAccessMatch.ip_refs[] | Fixed ConfigData; actual condition/connection owner |
| Gateway.metadata.annotations[edgion.io/inbound-proxy-protocol].resourceRef, source=resourceRef | Fixed ConfigData via JSON tagged annotation; required name, optional namespace defaults Gateway-local; existing ProxyProtocolTrustResourceRef validation and visibility |
| DynUpstreamTarget | Omitted namespace matches route backends by name across namespaces; selector, not owner-local lookup |
| HTTP/gRPC route delegation backendRef name '*' | NamespaceRoutes selector with exact supported group/kind and reverse consent; not a fabricated named object or Service capability |
| AiBackendSlotSelector / LogicalBackendSlotIdentity | Exact descriptor/occurrence dimensions; canonical empty group, not core label |
| Parsed ${secretRef:ns/name:key} | Fixed Secret; namespace required by existing parser; actual template binding source |
| Gateway.metadata.annotations[edgion.io/edgion-stream-plugins] | Fixed EdgionStreamPlugins; single trimmed local name, slash rejected, blank absent; Gateway namespace/default fallback; preserve existing registration and applicable listener stages |
| TCPRoute/TLSRoute/UDPRoute.metadata.annotations[edgion.io/edgion-stream-plugins] | Fixed EdgionStreamPlugins; single trimmed value; slash-containing key retained, bare name route-local/default; preserve existing per-route stages; no new Controller grant/registration behavior |
| Gateway.metadata.annotations[edgion.io/edgion-plugins] | Fixed EdgionPlugins; split on comma, space, tab or LF, then trim tokens; preserve ordered deduplication; bare names Gateway-local/default; slash-containing keys retained; no equivalent HTTPRoute/GRPCRoute annotation consumer |
| Gateway.metadata.annotations[edgion.io/dns-resolver-ref] | Fixed LinkSys/httpdns; explicit namespace/name, existing trimming and extra-slash rejection; Gateway source for existing cross-namespace Grant policy |
| Service.metadata.annotations[edgion.io/dns-resolver-ref] | Fixed LinkSys/httpdns; same string grammar but core Service source; existing cross-namespace Grant policy; Controller DNS producer and Gateway Service resolution |
| Controller YAML dns_resolver.link_sys_ref | Fixed LinkSys/httpdns string; process-config provenance, explicit namespace/name; direct Kubernetes API consumer with existing kube-less system-resolver fallback; no invented resource-owner/Grant source |
| BackendTLSPolicy options edgion.io/client-certificate-ref | Fixed same-namespace Secret name; no slash/cross-namespace input |
| Service.metadata.annotations[edgion.io/gateway-service-ref] and companion gateway-service-uid | Fixed namespace/name Gateway binding plus exact UID; dedicated Service discovery verifies both and preserves UID-derived Service naming |
| EDGION_GATEWAY_SCOPE and optional EDGION_GATEWAY_UID | Fixed process-instance Gateway locator; existing namespace/name grammar and UID-requires-scope checks; preserve scoped publication and same-name replacement protection |
| EdgionAcme.spec.storage.secretName/secretNamespace | Core Secret write destination; required name, omitted namespace uses Acme namespace with existing owner-context default fallback; preserve publication authority and target UID/resourceVersion fencing |
| EdgionAcme.spec.autoEdgionTls.name | EdgionTls generated output identity; omitted name acme-{Acme.name}; namespace is Acme namespace, independently of certificate Secret namespace |
| CredentialSource persist-on DesiredCredentialConsumer.target_secret_namespace/name | Internal generated core Secret at consuming EdgionPlugins namespace/name, independently of source LinkSys namespace; parse/readonly read and manager read/create/patch/delete |
| Generated credential Secret ownerReferences and credential-consumer/source annotations | Existing EdgionPlugins owner plus source/consumer UID/generation, authorization/binding/input/publication revisions and leader fence; scoped InstanceBinding, never name-only ownership |
| CredentialSource persist-off active memory identity | Existing LinkSys namespace/name/UID key, one active publication; preserve expiry, mode transitions and independent retained Arcs |
| EndpointSlice.metadata.labels[kubernetes.io/service-name] plus metadata.namespace | Fixed core Service association; Controller requeue and Gateway discovery; no operator group/kind or ReferenceGrant edge; preserve field-specific fallback differences below |
| DNS-generated EndpointSlice metadata.name/namespace/Service ownerReference | Internal GeneratedOutput; Service namespace; names {Service}-edgion-dns / {Service}-edgion-dns-ipv6; exact existing Service-UID ownership, forced SSA and GC |

The EGC dnsResolver.linkSysRef object requires both namespace/name and is a
data-plane default. It is separate from Controller YAML's snake_case string
reference and process authority; neither acquires an invented resource namespace
or Grant source. String profiles preserve their actual existing grammar and
provider-kind/failure checks rather than imposing one universal string parser.

Generated credential parse and readonly/status share existing managed-marker,
matching-owner, consumer/source UID, source-generation, computed binding-revision
and expiry checks. They do not acquire manager-side leadership or CAS checks.
Manager operations separately preserve exact ownership, current-source/consumer
checks, leadership validation, publication/input revisions and POST/JSON-Patch
CAS; cleanup keeps Secret UID/resourceVersion preconditions. Existing reader
owner matching and manager exact-owner policies remain distinct. Same-name
replacement or conflicting Secret never gains authority through ns/name
normalization. Metadata ownerReferences are inventoried only in these existing
binding/output protocols, not added as a general lookup feature. Identity sharing
does not make reader/writer acceptance or authorization policies identical.

EndpointSlice missing-label/namespace handling is not currently uniform:
Controller ignores a missing Service label and defaults absent namespace to
default; Gateway falls back to slice.name and retains an unqualified key when
namespace is absent. Preserve these existing policies in field adapters or obtain
an explicit impact decision before changing them. Valid Service association is
same-namespace. Relabel/replacement must rebuild both old and new Service pools.
DNS outputs preserve last-good slices on resolution failure, two-family naming,
Service-UID ownership, forced SSA and existing garbage collection. This does not
enable direct EndpointSlice backendRef support.

Instance bindings carry UID only where the existing protocol requires it;
do not create a global UID requirement for ordinary object references or retained
TLS generations. Dedicated Service annotations are produced by
cicd/deploy/kubernetes/gateway/provision-instance.py and consumed by
lookup_dedicated_gateway_service_addresses. Preserve cross-language binding
serialization with parity fixtures; the Python producer cannot import the Rust
module. This does not add provisioning functionality or material access authority.

ACME output identities are WriteDestination/GeneratedOutput adapters, not
read-only SecretLookup capabilities. Preserve attempt/Lease/source fencing,
operation-specific authority, existing-target UID/resourceVersion preconditions,
forced SSA and account-Secret merge-patch concurrency rules. The account Secret
reference covers read/create/patch operations; EAB strict identity validation
alone is not evidence of EAB ReferenceGrant enforcement. Reuse identity primitives
without asserting new authorization parity or changing side-effect policy.

ConfigData visibility Cluster retains its current grant exemption; namespace
visibility uses its existing policy. Global plugin default namespace is a named
field exception, not a default for cluster material or arbitrary owner contexts.

## Classified exclusions

The inventory covers implemented resource/config reference consumers; it does
not turn every string or Kubernetes metadata reference into a new lookup API.
Explicitly outside this migration: service-owned conf_sync CA/leaf Secret and
leader-election Lease bootstrap/ownership protocols; generic ObjectMeta
ownerReferences; unused EndpointSlice endpoints[].targetRef; Controller etcd
bootstrap SecretSlot/synthetic conf-center-ca placeholders, which select loaded
file material rather than Kubernetes resources; external OCI references, files,
DNS/network addresses and request/header/session identifiers. Gateway TLS
cert-provider=EdgionTls is a provider selector and expose-client-cert is a boolean,
not a named object reference. Namespace label selectors retain their own policy
and watch path. Direct reads/writes outside the frozen reference-field scope do
not make this task a bootstrap PKI, leader-election or general metadata redesign.

EdgionPlugins ExtensionLocal, globalPluginsRef and Gateway plugin annotation
targets use the EdgionPlugins-specific missing/unique/ambiguous source contract
in 01-spec. Raw namespace omission remains authored; explicit empty namespace
is not an alias. This does not extend default aliases to other target kinds.

### GatewayClass parameters owning adapter

`refs::gateway_class_fields::GatewayClassParametersRef` borrows the actual class
and validates its optional parameters field before Controller, Gateway startup,
or scoped publication reads a target. An absent reference retains built-in
defaults. A present reference requires exact `edgion.io/EdgionGatewayConfig`, a
valid name and an unset namespace (explicit empty is invalid). The target is
cluster-scoped; no ReferenceGrant or material capability is introduced. Invalid
references report Controller `InvalidParameters`, skip Gateway startup admission,
and publish an unavailable runtime scope. Only valid references retain the
Controller's unwatched-EGC unknown/Skip behavior. Existing EGC/class requeues and
snapshot recovery remain unchanged; this does not reconstruct live listeners.

## Policy target owner identity

The actual BTP/EBTP field loans in `refs::policy_fields` resolve omitted owner
namespace to `default`; explicit empty namespace is invalid. Raw policy keys,
queue ownership and exact deletion remain unchanged. Controller winner/status
checks use the raw policy key separately from semantic target identity.

Malformed EBTP target identities reject the whole policy before target or BTP
hostname reads. Canonical valid siblings retain recovery dependencies and ancestor
identity, but no partial conflict eligibility or resolved marker is published.
Service fan-out/duplicates remain unrestricted; only EdgionBackend targets carry
the existing 16-target and uniqueness limits. BTP still requires one eligible
target and preserves invalid exact/whole occupancy for canonical target slots.
Gateway target indexes share these loans without changing atomic publication,
precedence, captured snapshots, or TLS/material ownership.

R5c1 closes FixedConfigData `IpAccessMatch.ip_refs[]` in the effective condition
roots above and RAP direct IP rule fields through `PreparedConditionReferences`.
Tokens bind actual owner, placement and authored name/namespace; Controller-owned
resolved payloads and denial markers do not change descriptor identity. These
synchronous loans grant no Secret material role or asynchronous frozen authority.
R5c2 closes standalone HTTP IpRestriction, connection IpRestriction, TLS-route
IpRestriction and every GCIR profile's allowRefs/denyRefs using actual owner loans.
Invalid slots contribute no lookup, dependency or Grant query; valid missing and
denied slots retain recovery edges under raw owner keys. GCIR activeProfileRef,
At the R5c2 boundary, non-IpList roles remained follow-up work. WAF and the finite remaining Selector/URL/Region slots are covered by the later R5d/R5e rows below.

### R5e remaining plugin ConfigData slots

| Actual owning field | Target role / namespace | Permission source | Failure and status |
| --- | --- | --- | --- |
| EP.requestPlugins[*].RequestAccessPolicy.profiles[*].requiredRuleGroups[*].anyOfRules[*].Url.configRefs[*] | RequestAccessUrlAllowList; omitted namespace uses effective EP namespace | Actual EP, including imported conditions | Partial-good inline/sibling preservation; parse-time bounded PartiallyInvalid warnings |
| EP.requestPlugins[*].RequestAccessPolicy.activeProfileRef | Selector; effective EP namespace | Actual EP | Configured failure denies via RAP filter; current readonly ResolvedRefs |
| EP.requestPlugins[*].Canary.activeProfileRef | Selector; effective EP namespace | Actual EP | Validated base fallback; current readonly ResolvedRefs |
| EP.requestPlugins[*].RegionRoute.overrideRef | RegionRouteOverride; effective EP namespace | Actual EP | Validated base fallback; enabled-entry readonly ResolvedRefs |
| ESP.plugins[*].GlobalConnectionIpRestriction.activeProfileRef | Selector; effective ESP namespace | Actual ESP | Validated base fallback; current readonly ResolvedRefs; disabled entries retain existing diagnostics/dependencies |

All five fields reject invalid owner/target identity before lookup, Grant query or edge creation. Only target namespace omission defaults; raw target store keys are not aliased. Raw source queue/delete identity remains independent.

R6a closes the Controller subset of the Service/Gateway DNS annotation rows:
actual borrowed sources and canonical finite grammar: identity/query construction
precedes dependency registration, current permission checking and target acquisition. Process defaults retain separate authority.
The DNS GeneratedOutput row checks every required family and current Service UID
before skipping an unchanged successful publication. Gateway cold DNS migration,
other fixed consumers, async freshness and obsolete-family deletion remain open.

R6b closes Gateway cold consumption for Service/Gateway DNS annotations and
EGC.spec.dnsResolver.linkSysRef. The Service projection, GatewayInfo and existing
EGC DNS snapshot own cold candidates derived from actual resources. EGC typed
fields require explicit checked object namespace/name; they do not use the finite
annotation string exception. Administrative EGC selection adds no Grant source.
The raw-key winner is selected before validation and retains independent TTL; an
invalid configured ref yields System without selecting another EGC or inline
servers. Inline-only configuration is not an object-reference authorization.
Live provider lookup and all remaining reference-family work are unchanged.

### Actual HTTP attachment field binding (C1)

EGC globalPluginsRef and Gateway HTTP annotation consumers now use `refs::plugin_attachment_fields::HttpPluginAttachments` before registration/classification and cold publication. EGC targets default to `default`; Gateway bare tokens default to the effective owner namespace. Raw owner namespace omission is preserved in dependency identity. Invalid slots have no usable key or external lookup; valid missing/ambiguous slots retain recovery edges. Publisher, GatewayInput, and listener metadata share the adapter and retain existing Missing/drop versus Ambiguous/denial behavior. No ReferenceGrant policy is introduced for these attachment fields.

## Actual stream attachment owner boundary (C2)

C2 coverage: Controller Gateway registration/status, GatewayInput, TCP/TLS/UDP RouteInput and all three route-unit constructors use StreamPluginAttachment. The thin Gateway projection preserves invalid as configured; publisher guards sentinel demand/reads. Raw dependency owner namespace omission remains intact; route units still independently reject invalid backend owners. See the [canonical stream attachment contract](../../../skills/04-review/plugin-runtime/intentional-design/stream-plugin-store-key-normalization.md).

### Actual inbound PROXY reference field (C3)

Gateway.metadata.annotations[edgion.io/inbound-proxy-protocol].resourceRef is consumed through `GatewayProxyProtocolField`: actual bounded parse once, listener selection/inline first, then checked Gateway-to-EdgionConfigData query, diagnostic identity, dependency registration, target acquisition and canonical visibility/Grant decision. Effective default namespace never rewrites raw dependency owner identity. Invalid source has zero edges/reads; valid missing/denied targets preserve requeue recovery. Gateway continues to consume only the resolved trust snapshot.

### Process output boundary (R6f)

`output::LinkSysRef` in StringOutput ES/Kafka/OTLP has explicit namespace/name,
canonical checked DNS identity, and a fallible key at cold sender construction.
It has process provenance, no fabricated Kubernetes owner or Grant. Async sender
logging and ES backfill capture consume only checked keys; live typed lookup is
unchanged. See the [LinkSys DataSender contract](../../../skills/01-architecture/02-gateway/09-link-sys.md#datasender).

Remaining fixed resource-owned coverage explicitly includes
`GraphqlRateLimit.redisRef`: GraphqlRateLimit::create converts rate_limit_config()
into RateLimitRedis::new. A later actual-field adapter must retain that original
Graphql source, not only match the RateLimitRedis plugin variant. This batch does
not claim those resource-owned consumers migrated.

### HTTP fixed LinkSys owner binding (R6g)

DSL linkSysRefs (request and response-filter), RateLimitRedis.redisRef,
GraphqlRateLimit.redisRef, WebhookKeyGet vars[].webhookRef, and AiProxy
tokenQuota.redisRef now participate in the actual EdgionPlugins prepared
topology and cold configuration loan. Original lexical validators and per-use
typed provider lookup remain authoritative. Graphql delegation retains its
original config source. No ReferenceGrant is invented for these fields.
Stream ConnectionRateLimit.redisRef and EdgionBackend credentialPool.redisRef
remain separate pending actual-owner consumers.

### Stream fixed LinkSys owner binding (R6h)

ConnectionRateLimit.redisRef consumes an actual EdgionStreamPlugins ordinal/config
loan before cache reuse only when perSourceIp or distributed perListener makes
Redis active. Disabled/policy-masked entries and local-only unused refs retain
their existing behavior. Target grammar remains exactly two nonempty components
using Unicode alphanumeric/-/_. EdgionBackend credentialPool.redisRef remains a
separate pending boundary: its existing grammar is ASCII alphanumeric/-/_ with
two nonempty components, and its UID/pool-equivalence lifecycle is not changed.

### Backend fixed Redis source binding (R6i)

EdgionBackend credentialPool.redisRef now borrows its actual owning field after
canonical schema validation and before UID/credential/runtime-pool construction.
Some requires intrinsic source identity; None remains inactive. Target ASCII
grammar, limits pairing, live provider lookup and existing pool/UID/captured-view
semantics stay unchanged. This completes only the fixed LinkSys field; separate
Backend Secret material ownership is not implied and remains an independent
verification candidate.


### R6j process locator reconciliation

Gateway startup and parent-based Route/EdgionTls cold projection share the finite
GatewayProcessLocator grammar. Exact scope/UID bytes, UID-first startup errors,
shared mode and same-name replacement protection remain; malformed direct parent
projection is rejected. EdgionAcme raw attachment-key equality is unchanged.
Controller process DNS already reaches canonical DnsLinkSysKey through LinkSysKey;
its Kubernetes typed HTTPDNS acquisition and kube-less System fallback remain
process authority, without a fabricated Kubernetes source or Grant. Dedicated
Service exact reference-plus-UID checks and cross-language UID-derived naming are
separate existing ownership guards. Generated output authority and Backend Secret
actual-source verification remain separate work, not covered by this completion.

### R6k Backend credential Secret fields

Controller credentialPool.credentials[*].secretRef now uses an actual Backend
ordinal loan into the shared bound Secret pipeline. Invalid source/target identity
precedes all new dependencies, authorization and Secret reads; valid missing/denied
references retain recovery wiring and partial-good siblings. The existing custom
core/Secret profile, native namespace omission, raw dependency owner keys and
material redaction remain. Controller-only InvalidReference status classification
does not add schema fields. This is not closure of generic credential consumers,
Gateway arbitrary resolved-material trust, generated output authority or R7/R8.

### Managed plugin Service and local TLS ownership

The R7a scope migrates enabled request ForwardAuth.target, Guardrail.target and
AiGuard.evaluator.target to actual EdgionPlugins entry loans, together with their
existing local TLS selection. Omitted Service namespace now inherits that owner;
explicit empty namespace is invalid. Shared validation also checks the same
finite custom Service shape consumed by LinkSys Webhook and internal Edgion auth:
omitted/empty/core group denotes core Service; custom transport modes remain
separate from resource kind. This changes no native Gateway API field profile.

Owner/target/active TLS validation precedes cached executable reuse and client
construction. Disabled/policy-masked entries and inline-only AiGuard acquire no
new outbound reference. Service TLS follows enabled rather than block presence;
explicit HTTPS URL transport remains secure when custom local TLS is disabled.
No Service Grant, dependency or forwarding-policy relationship is introduced.
R7a excluded internal HTTPRoute Edgion-auth (covered by the R7b boundary below),
CredentialSource transport capture (covered by R7c below) and generated-output
operation authority. Generated-output authority remains open. R7a focused handler/store tests did not establish live
queue delivery or conformance.

### R7a managed-template readiness follow-up

The three managed plugin preparations now share the internal Route adapter's
private exact-key presence predicate. Actual request templates require their
controller-carried `namespace/name:key` entries before TLS/client/health work,
cache replay and publication; an existing empty value is accepted. Missing material
uses the existing non-retainable failure path instead of the renderer's empty
substitution. Disabled/policy-masked entries and inline-only AiGuard retain their
activity semantics. This adds no Grant, freshness, schema or Service dependency
claim, and does not migrate Webhook, OAuth2 or CredentialSource. Focused evidence
covers owning construction, cache challenge, replacement/repair/deletion and
captured prior Arcs; live verification remains separately tracked.

### R7b internal HTTPRoute Edgion-auth boundary

Internal FS/etcd auth filters now have actual authored rule/backend/filter loans
and separate effective locality. Controller root/child declarations resolve local
TLS CA/client and request-template material through canonical bound acquisition;
private valid-root receipts own Secret/ConfigMap/Grant recovery. The declaration,
not the published root, supplies authorization and default Service namespace.
Incoming resolved material and namespace stamps are scrubbed before publication.
Readonly status has no new dependency writes. Gateway construction retains the
actual effective field through canonical TLS identity proof and uses Service TLS
`enabled` semantics while preserving explicit HTTPS URLs and existing client
behavior. Effective locality has no authored token/Grant/freeze API.

This migration does not implement upstream ExternalAuth, change native field
admission, add Service grants/dependencies, migrate OAuth2, or complete R8 and
live/conformance verification. Tests include root event-queue recovery and
multi-level/backend locality; final pass evidence remains in the active batch
verification record rather than inferred from this inventory.

### R7c Controller OAuth2 owning transport

CredentialSource now retains one private actual LinkSys owner, with authorized
TLS material moved after the scrubbed owner's cold clone. Public conf_sync stays
free of that payload. Registry clones share the owner Arc. The async manager
captures PreparedLinkSysTls before DNS await; OAuth2 derives its configuration
from the same owner and consumes the existing canonical proved HTTP factory.
Active local/global pairing precedes client construction. Disabled local policy
remains inert while explicit HTTPS stays secure. Existing raw-owner/frozen-input,
Grant/Secret revision, issuer/fallback, leadership and publication fences are
unchanged. This completes OAuth2 transport capture, not generated-secret operation
authority, a live issuer handshake, or Gateway API conformance validation.

### R8a native Gateway TLS acquisition

Gateway.spec.tls.backend.clientCertificateRef, listener certificateRefs and
spec.tls.frontend.default/perPort validation.caCertificateRefs use actual borrowed
Gateway field loans and the existing bound credential pipeline. Native omitted
Secret defaults remain empty-group/Secret; frontend group/kind stay required;
`core` is not a native alias. Native names preserve the schema's character-count
bound. Omitted owner namespace means default; an omitted target namespace uses the
effective owner namespace. Controller acquisition and Gateway matcher compilation
share those defaults without manufacturing owner metadata.

Invalid identities create no authorization/read/dependency effects. Valid missing
or denied references retain the correct ordinary Secret or typed frontend CA
recovery edge. The disabled controller Grant policy remains limited to finite
Gateway positions. Frontend fallback authorizes without payload reads; strict
CA bounds and complete-set matcher semantics remain unchanged. Controller keeps
ordered serving outcomes, not permission to serve a partial set. Status is
read-only, and input material is scrubbed before inactive/rejected processing.

BTP, EBTP, EdgionTls, ACME and generated-output authority are not closed by R8a.
Focused tests and matcher rebuild evidence do not replace the remaining real
environment and Gateway API conformance validation.

Native Gateway TLS material selection preserves FS/etcd raw omitted namespaces:
a canonical default target checks both qualified and bare candidates after
validation and authorization, rejects ambiguity or mismatched actual metadata,
and retains the selected object's metadata. Nondefault targets never read bare
keys. Other bound credential consumers retain their existing exact reads.
Secret/ConfigMap source events preserve raw dependency notifications and add
Gateway-only semantic ordinary notifications for bare objects, plus correctly
typed frontend CA notifications. The two candidate reads are not an atomic
snapshot; dependencies are registered first and subsequent events reconcile
creation, deletion, revocation and ambiguity changes. Matcher identity comparison
interprets only an omitted actual Secret namespace as default, without rewriting
material metadata or accepting an explicit empty namespace.
