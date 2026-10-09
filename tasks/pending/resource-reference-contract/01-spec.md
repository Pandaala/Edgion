# Shared resource reference specification

## Status and authority

Reviewed design after three general Astra rounds and two additional focused
rounds in each of three directions. Implementation is authorized by
[00-context.md](00-context.md) and in progress; fresh scoped checks are recorded in
[04-implementation.md](04-implementation.md). Full-manifest final review and live
conformance verification are not complete. Observed code in the original design
inventory describes the pre-implementation baseline. The user requested a
general reference module, broad inventory, support for common omitted-field
inputs, evaluation of compatible conveniences, and multiple Astra reviews.
The exact proposed field registry is [02-field-manifest.md](02-field-manifest.md).
An earlier proposal to remove every core alias is superseded as a proposal by
this broader design investigation.

The repository [design rules](../../../skills/00-design-rules.md),
[schema and transport contract](../../../skills/01-architecture/05-resources/00-schema-transport-contract.md),
and the vendored Gateway API v1.6.2 schemas govern implementation. New conveniences
are product proposals, not claims about Gateway API defaults. The specification
does not automatically relax existing ACME/TLS rules. The explicit implementation
approvals in 00-context supersede historical confirmation gates below.

## 1. Problem and desired behavior

Current reference handling mixes field defaults, accepted targets, alias handling,
authorization identities, dependency keys, and runtime lookups. Shared
is_core_api_group accepts both empty and core, while several consumers duplicate
that predicate and others reject core. ReferenceGrant merges the two identities.
Generic Secret fetching ignores authored group/kind entirely. Some standard and
Edgion-specific references share a wire type despite different required fields
and namespace rules.

The goal is one canonical interpretation for each declared reference field,
reused from validation through actual consumption. Omitted fields remain easy to
use where the field contract permits omission. Explicit values never disappear
because another layer guesses the intended resource. Standard fields and
Edgion-owned convenience fields may have different input grammars, but a resolved
identity and authorization decision cannot change between consumers or sources.

## 2. Evidence from current code and tests

### 2.1 Definitions and shared implementations

Paths in this table are relative to edgion-resources/src/resources unless noted.

| Family | Current definition and behavior | Design consequence |
| --- | --- | --- |
| HTTP backend | http_route.rs, HTTPBackendRef; group/kind/namespace are optional | Omitted group defaults to empty, kind to Service; preserve raw omission |
| gRPC/L4 backends | grpc_route.rs, tcp_route.rs, tls_route.rs, udp_route.rs | Share identity defaults; keep protocol-specific target support |
| Native mirror | http_route.rs, BackendObjectReference; group is defaulted String | Already rejects non-empty group; share identity without changing mirror failure scope |
| Route filter ExtensionRef | http_route.rs, LocalObjectReference; group currently has serde default | Standard group/kind/name are required; omitted group must not infer edgion.io |
| Parent/attachment | common/mod.rs and common/parent_identity.rs | Existing borrowed canonical identity correctly separates omitted and empty group |
| Allowed route type | gateway.rs, RouteGroupKind | Optional group defaults to Gateway API group; this is a type declaration, not an object lookup |
| Secret/credential | common/secret_object_ref.rs, SecretObjectReference | Optional group/kind; reused for core Secrets and CredentialInjector LinkSys targets |
| Standard policy target | backend_tls_policy.rs, BackendTLSPolicyTargetRef | Group/kind/name required; same namespace; preserve sectionName |
| Edgion policy target | edgion_backend_traffic_policy.rs, PolicyTargetRef | Group can be omitted; same identity classifier, different input contract |
| CA reference | backend_tls_policy.rs, BackendTLSPolicyCACertificateRef | Currently mixes standard local required fields and Edgion optional-group/cross-namespace fields |
| Frontend CA | gateway.rs, FrontendCACertificateRef | Existing separate strict wire type is a useful precedent |
| GatewayClass parameters | gateway_class.rs, ParametersReference | Group/kind/name required; current supported EdgionGatewayConfig target is cluster-scoped |
| Grant source/target | reference_grant.rs | Required group/kind; matching must preserve exact identities |
| Webhook Service target | common/external_http.rs, WebhookServiceRef | Optional fields permit URL alternative; Service branch requires a target; group is currently not validated |
| AI logical slot | http_route.rs, LogicalBackendSlotIdentity | Currently converts empty/omitted group to the string core; do not confuse an internal label with API identity |

Existing reusable foundations are ParentReference::canonical_identity,
validate_extension_ref_filter, HTTPBackendRef::backend_kind, the mirror target
predicate, policy canonical keys, and the Controller dependency-before-grant
helper. Move or adapt these mechanisms; do not create competing implementations.

### 2.2 Fixed-target references and selectors

Gateway.spec.gatewayClassName is a fixed cluster GatewayClass reference, with
required name and no namespace or ReferenceGrant. Controller class ownership,
Gateway scoped publication, and class lifecycle consumers share that adapter.
GatewayClass.controllerName is a controller identifier, not an object ref.

EdgionConfigDataRef, ProxyProtocolTrustResourceRef, WafBundleRef, plugin refs,
LinkSysRef, DNS resolver annotations, stream-plugin annotations, template Secret
references, redisRef, and webhookRef have fixed target types or existing string
grammars. They participate through typed adapters; they do not need new operator
group/kind fields. LinkSysRef.name also serves as a DSL alias, which is a separate
contract from resource identity. The distinct output::LinkSysRef used by
StringOutput Es/Kafka/Otlp sinks requires namespace/name and existing DNS
validation; LogConfig output/fallback bindings retain provider-kind checks and
binary-owned log lifecycle without acquiring a new ReferenceGrant policy.

Dedicated Service annotations gateway-service-ref and gateway-service-uid bind
a Gateway namespace/name plus UID. lookup_dedicated_gateway_service_addresses
validates both and derives the Service name from that same binding. Process
EDGION_GATEWAY_SCOPE/EDGION_GATEWAY_UID locators preserve their scoped-publication
and replacement guards. These fixed bindings share identity primitives without
adding group fields, material authority, or a universal UID requirement.

ACME storage Secret and autoEdgionTls name are write/generated-output identities.
The Secret namespace defaults from Acme context; generated EdgionTls stays in
the Acme namespace even when the Secret namespace is explicit and different.
Reuse operation-specific identity adapters while preserving publication fences,
target UID/resourceVersion and write authority. They are not SecretLookup reads.

CredentialSource has another existing generated binding: persist-on publishes a
Secret at the consuming EdgionPlugins namespace/name, independently of the source
LinkSys namespace. Parse/readonly/status share current managed-binding/expiry
checks; manager operations separately retain exact ownership, publication/input
revisions, leadership and mutation fences. Their policies do not become identical.
Persist-off uses the source LinkSys namespace/name/UID active-memory identity.
Do not infer ownership or publication authority from deterministic names alone.

EndpointSlice-to-Service label association and DNS-produced slice identities
are also fixed/internal categories. Preserve same-namespace valid associations,
old/new pool rebuilds and existing malformed-input fallback differences. These
associations are not direct EndpointSlice backendRef support. Exact process DNS,
Redis/Webhook LinkSys and annotation fields are listed in the finite manifest,
including distinct Gateway versus route stream-plugin grammars.

DynUpstreamTarget.namespace omission means match route backends by name across
their namespaces. It is not an owner-namespace default. ReferenceGrant.to.name
omission is a wildcard for the selected group/kind. AI slot occurrence, section,
and candidate order are selector dimensions. Do not normalize any of these into
ordinary object lookup defaults.

Existing named/wildcard HTTPRoute and GRPCRoute delegation is another separate
Edgion extension boundary. Backend name '*' enumerates routes in the selected
namespace with reverse attachment consent; it is not an object literally named
'*'. The permissive vendored name schema alone does not establish standard
BackendObjectReference semantic conformance. Preserve this established behavior
without claiming that every existing delegation input is a standard reference.

### 2.3 Consumer gaps that the migration must close

- Controller credential/pipeline.rs::fetch_secret_inner registers dependencies,
  checks grants as empty-group Secret, and reads SecretStore without examining
  the input group/kind. Lenient validation does not stop this path.
- EdgionBackend credential parse, several plugin resolvers and status helpers,
  and OAuth2 bootstrap resolution can call that generic pipeline. OTLP and
  CredentialInjector already have useful pre-consumption guards.
- HTTP/gRPC/L4 backends, policy targets, CA resolution, global TLS, and plugin
  validation use a mix of independent predicates and the shared core alias.
- Gateway gRPC compile_policy_target_key validates group only for a policy key.
  routes/http/proxy_http/pg_upstream_peer.rs::init_backend_context_if_needed
  constructs the selected gRPC backend context from raw kind without a group
  gate. A unified policy key alone does not protect Service lookup or dialing.
- ExtensionRef already has shared Controller/Gateway target and union validation.
  Preserve it across rule/backend scopes, delegated children, and preflight.
- Gateway TLS material validation binds references to resolved source identity.
  Reusing a group helper does not replace source/material binding checks.
- Inline ForwardAuth/Guardrail/AiGuard Service namespace omission currently reaches
  ManagedOutboundClient without owner context and defaults to default; LinkSys
  webhook already injects its owner namespace. Proposed inline owner-local
  semantics are a correction, not a claim about current uniform behavior.
- ForwardAuth has local TLS wire fields consumed by its outbound client, but its
  Controller resolver currently handles templates rather than local CA/client
  material. The field manifest marks this wiring gap; do not advertise existing
  functioning material resolution from wire presence alone.

These are current-code observations, not reports that an exploit or live outage
was reproduced. Earlier investigation inferred too much from the gRPC policy
key; this deeper call-chain inspection establishes the narrower evidence.

### 2.4 Existing fixtures favor valid omission

A read-only YAML survey of edgion-tests/integration found 1,730 YAML files and
2,030 documents with no parse errors. Recursive reference occurrences were
counted, including repeated and dynamic fixtures; counts are not production
usage or executed-test results.

| Reference occurrence | Omitted group | Empty group | Other group | Explicit core |
| --- | ---: | ---: | ---: | ---: |
| HTTPRoute backendRefs | 628 | 1 | 42 | 0 |
| HTTPRoute parentRefs | 511 | 0 | 3 | 0 |
| HTTPRoute ExtensionRef | 0 | 0 | 373 | 0 |
| HTTPRoute native mirror | 4 | 0 | 0 | 0 |
| GRPCRoute backendRefs | 36 | 0 | 4 | 0 |
| GRPCRoute parentRefs | 26 | 0 | 4 | 0 |
| GRPCRoute ExtensionRef | 0 | 0 | 7 | 0 |
| Gateway certificateRefs | 32 | 1 | 0 | 0 |
| BackendTLSPolicy targetRefs | 0 | 29 | 0 | 0 |
| BackendTLSPolicy CA refs | 0 | 25 | 0 | 0 |
| EdgionBackendTrafficPolicy targetRefs | 33 | 1 | 1 | 0 |

Existing minimal Secret tests also construct group/kind as None. Core aliases
are accepted in some unit tests, but the surveyed integration references do not
depend on them. Preserve omission fixtures rather than making all values explicit
to hide defaulting regressions. ExtensionRef and standard policy references
already use their required explicit fields.

### 2.5 Kong and Envoy Gateway comparison

This is source inspection at fixed commits, not a claim about every release or
executed product behavior. The source links in section 9 identify the revisions.

| Implementation | Defaults and target validation | Authorization and design lesson |
| --- | --- | --- |
| Kong Kubernetes Ingress Controller | Backend group helper maps nil/empty to an internal core label and also accepts the literal core for Service matching | Common ReferenceGrant matching is exact; other certificate/status/watch paths differ. Reusing a helper alone does not establish end-to-end identity consistency |
| Envoy Gateway | Backend/Secret validation preserves non-empty group identity and does not accept core as a core-group alias; local resource loading applies schema defaults | Exact ReferenceGrant matching is a separate operation. Field defaults and supported target classification can be centralized without rewriting explicit group values |

Our proposal follows the standard field identities, keeps authorization exact,
and makes any Edgion-only shorthand a declaring-field conversion before every
consumer. Peer behavior is comparative evidence, not authority to widen standard
reference syntax or grant matching.

## 3. Standard and convenience input contracts

### 3.1 Standard profiles

An explicit empty group is the Kubernetes core group. The literal core is a
non-empty API group, potentially identifying a distinct registered resource.
It never becomes empty in a standard Gateway API reference or ReferenceGrant.

| Standard field family | Omitted group | Omitted kind | Namespace | Core alias |
| --- | --- | --- | --- | --- |
| Route backendRef and native mirror | Empty | Service | Owner namespace if omitted; cross-namespace authorization as applicable | No |
| Gateway certificate/client Secret ref | Empty | Secret | Owner namespace if omitted; grant-gated cross-namespace | No |
| ParentReference | gateway.networking.k8s.io | Gateway | Referring resource namespace if omitted | No |
| ExtensionRef LocalObjectReference | Missing required field | Missing required field | Declaring route local; delegated origin preserved | No |
| BackendTLSPolicy CA LocalObjectReference | Missing required field | Missing required field | Policy local; operator namespace is not a standard field | No |
| BackendTLSPolicy target | Missing required field | Missing required field | Policy local | No |
| Gateway frontend CA | Missing required field | Missing required field | Follow its standard cross-namespace reference contract | No |
| GatewayClass parametersRef | Missing required field | Missing required field | Scope depends on supported target; EdgionGatewayConfig is cluster-scoped | No |
| allowedRoutes.kinds | gateway.networking.k8s.io | Missing required field | Not an object reference | No |
| ReferenceGrant from/to | Missing required field | Missing required field | Explicit from namespace; target is grant namespace | No |

Default only an omitted optional field. Explicit empty kind is not omitted and
does not become Service, Secret, or Gateway in standard references. Empty/invalid
namespaces and names are not defaults. JSON/YAML null is not an explicit empty
string. Kubernetes prunes null in non-nullable fields before applying defaults:
optional group/kind null therefore has the omission semantics of that field;
optional namespace null is pruned and uses the owner namespace. Required
group/kind/name without a default remain missing and invalid after pruning.
Nullable fields retain null and need their own schema contract. The narrow
reference adapters reproduce these effective semantics for the inventoried
source version and field; do not add blanket raw-null rejection or a new general
admission/defaulting engine. Existing Option null-to-None decoding is appropriate
for optional fields with these omission semantics.

The standard LocalObjectReference description mentions unspecified group, but
its required marker and committed CRD required list still govern admission.
Do not weaken those lists based on the description. Unsupported non-empty
groups remain intact and produce the owning consumer's unsupported-target
outcome; this design does not add support for resources in the group core.

### 3.2 Proposed Edgion convenience envelope

The user requested additional compatibility where it does not break Gateway API.
The recommended envelope is a documented field-specific Edgion input extension,
not historical compatibility shims or a global permissive switch.

Proposed initial profiles accept omitted group, empty group, or exactly the
lowercase literal core for a core-target branch. The canonical effective group
is empty in all three cases. Unknown non-empty groups retain their identity and
are rejected unless the field supports that exact group/kind.

| Edgion-owned field family | Proposed convenience | Constraints |
| --- | --- | --- |
| EdgionBackend credential pool Secret refs | core to empty; missing kind defaults to Secret | Validate before material access; no same-name fallback |
| EdgionBackendTrafficPolicy Service targets | core to empty; omitted group defaults empty | Kind remains required; edgion.io/EdgionBackend remains exact |
| EdgionBackendTrafficPolicy health-check CA/client fields | core to empty; omitted group defaults empty | CA Secret/ConfigMap and client Secret; policy namespace only, regardless of grants |
| EdgionGatewayConfig and EdgionPlugins outbound CA/client Secret fields | core to empty; omitted group defaults empty | CA kind required; cluster-scoped owner requires explicit material namespace |
| LinkSys auth, OAuth2 bootstrap, OTLP and outbound TLS fields | core to empty; omitted group defaults empty | Secret branch only; every lookup and readonly status path uses the profile |
| Plugin Secret fields and CredentialInjector credentialRef | core to empty on Secret branch; missing kind defaults Secret | LinkSys branch requires exact edgion.io/LinkSys, never inferred from name/kind |
| Exact outbound HTTP Service fields in the manifest | core to empty on core branch | URL and Service branch validation remains separate; retain documented custom kind grammar |

ACME account/EAB and EdgionTls server/mTLS references retain their currently
accepted strict group contracts. Expanding these is not necessary to share the
module and would require a separate explicit product decision. Standard fields
inside Gateway, HTTPRoute, GRPCRoute, L4 routes, BackendTLSPolicy, and ReferenceGrant
never receive this convenience, regardless of configuration-center mode.

This table is an explicit proposal for review, not an assertion that every listed
field already accepts core or that the product decision is approved. During
implementation planning, freeze the linked finite field-path manifest against
the current schema and caller audit. No convenience may ship from a generic
shared wire type or an unlisted caller; unknown fields remain strict.
Preserve existing custom-only empty-kind conveniences only where the field's
current documented contract explicitly specifies them, such as Webhook Service
selection; do not copy them into standard backend references.

### 3.3 Authorization after convenience conversion

Convenience conversion precedes target validation, dependency key construction,
authorization, material lookup, and runtime target compilation. All consume the
same canonical identity. A custom Secret ref with group core therefore requires
a grant target group of empty, not core.

ReferenceGrant itself always uses exact group/kind matching. A grant for group
core does not authorize a core Kubernetes Secret or Service, including an
Edgion convenience reference. The source side is equally exact. From namespace
and target namespace are exact; the target must be in the Grant namespace.
Only omitted to.name selects all names of that exact group/kind; explicit empty
name is invalid, not a wildcard. From has no resource-name selector. A match
requires any matching from AND any matching to in the same Grant, with OR across
whole Grants. Never combine from in one Grant with to in another. Object lookup
and authority-revision queries always carry the concrete validated target name.

Reference identity and ReferenceGrant matching share canonical GroupKind/scope
primitives. The existing ReferenceGrant::allows_reference implementation must
delegate to one pure exact matcher, rather than keep alias-aware matching beside
the new adapters. Controller store permission checks and authorization_revision
use that same matcher and bound source-target query. The pure module never
consults a store or decides whether this field requires a Grant.

No backend-specific core flag is allowed. The same declared resource and field
has the same effective identity in Kubernetes, FS, etcd, Admin, and trusted
conf_sync representations. Existing source schema/default application differs;
equivalence is about accepted field semantics, not bypassing structural schema.

### 3.4 Minimal input examples

Given an HTTPRoute in namespace app, these ordinary omitted fields remain valid:

~~~yaml
parentRefs:
  - name: public-gateway
rules:
  - backendRefs:
      - name: web
        port: 8080
~~~

The parent identity is gateway.networking.k8s.io/Gateway/app/public-gateway.
The backend identity is empty-group Service/app/web. Neither requires adding
group, kind, or namespace to existing fixtures. Backend port still follows the
Service target's contract; it is not a universal ref default.

A supported route filter has a different required-field contract:

~~~yaml
filters:
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: EdgionPlugins
      name: auth
~~~

It resolves locally to the declaring route's namespace, including delegated
children. Omitting group or writing core does not infer edgion.io.

For a proposed Edgion convenience Secret field, name-only input still means
empty-group Secret in the field's owner namespace. If that exact field is
approved for the shorthand, adding group: core produces that same effective
identity. A cross-namespace grant must still explicitly use to.group: "" and
to.kind: Secret. This shorthand example is not valid for standard Gateway Secret
references or the existing strict ACME/EdgionTls fields.

## 4. Shared module and API

### 4.1 Ownership

Add edgion-resources/src/resources/refs/. It contains pure identity/default/
target-classification code and typed adapters, with no stores, asynchronous IO,
locks, logging, metrics, Kubernetes clients, or dependency registration. Preserve
the workspace dependency DAG. Controller owns authorization and resource stores;
Gateway owns compiled targets and final consumption guards.

Controller retains the existing process-wide
ValidationConfig.enable_reference_grant_validation policy, default true, through
is_reference_grant_validation_enabled(). It belongs neither to refs nor to
EdgionGatewayConfig or a caller-selected field flag. Freeze its exact per-consumer
effect before migration: standard route/Gateway/mirror and some fixed-reference
checks consult it; credential Secret/ConfigMap fetch and CredentialInjector
LinkSys source checks currently do not. Do not silently harmonize those policies
or broaden bypasses. Any behavior change requires explicit impact approval.
When existing policy suppresses a Grant decision, identity/target validation and
dependency registration still apply. Grant-disabled operation cannot establish
standard Gateway API conformance.

Suggested internal files are identity.rs, contracts.rs, targets.rs, and adapters
grouped by semantic family. Avoid one file per wire struct when their contracts
are the same. Existing parent identity and ExtensionRef validation become part
of this module or delegate to it with a single owning implementation. Remove old
duplicate implementations and update callers, rather than keeping deprecated
re-exports for compatibility.

### 4.2 Borrowed identities and separate reference categories

Illustrative API shapes, not implemented declarations:

~~~rust
pub enum ResourceScope<'a> {
    Cluster,
    Namespaced(&'a str),
}

pub struct GroupKind<'a> {
    group: &'a str,
    kind: &'a str,
}

pub struct ObjectIdentity<'a> {
    target_type: GroupKind<'a>,
    scope: ResourceScope<'a>,
    name: &'a str,
}
~~~

Strings borrow the authored configuration or static defaults. A missing owner
namespace is not represented as a cluster-scoped target. Owner scope comes from
the owning resource's existing metadata/source contract; a cluster-scoped owner
cannot manufacture a namespaced target by globally choosing default namespace.

Keep ParentRefIdentity's sectionName/port and ParentStatusIdentity's controller
name as attachment-specific dimensions. Keep logical slot occurrence and
selector wildcard behavior separate. GroupKind type declarations reuse exact
identity rules without pretending to name an object.

Represent delegation as a separate DelegationSelector, for example
Named(ObjectIdentity) or NamespaceRoutes{GroupKind, namespace}. Recognize its
exact supported group/kind and selector contract before ordinary object-name
validation. NamespaceRoutes has no object name, grant target, policy target key,
or Service lookup capability. Preserve named/wildcard dependency tracking,
reverse consent, and root requeue ownership; resolved terminal object refs then
use their normal field contracts. This is a sealed field-specific extension,
not wildcard acceptance in a generic object parser.

Fixed instance bindings and generated/write destinations are also separate
categories. UID is a dimension of an existing instance binding, not ordinary
ObjectIdentity or a global retention gate. A WriteDestination names an output
and operation intent; it grants no read/write authority by itself and cannot
be passed as a SecretLookup. Generated names may be allocated once on the cold
configuration/publication path. ACME account refs cover read/create/patch, while
EAB identity checks do not imply an existing ReferenceGrant check. Preserve
attempt/source/Lease fencing, existing-target UID/resourceVersion, forced SSA
and account merge-patch concurrency rules; identity reuse cannot replace them.

### 4.3 Field contracts and adapters

Contract selection is static and owned by the declaring field adapter. Do not
expose a public normalize(group, caller_chosen_default, allow_alias_bool) API.
Each contract specifies required/default group and kind, namespace scope,
allowed target categories, optional section/port constraints, and any explicitly
listed Edgion convenience. A shared internal primitive implements omission and
exact comparison once.

Allowed targets and access scope remain per-field: LinkSys TLS CA supports only
Secret, including webhook/httpdns/OAuth2. Preserve TlsCertsResolver type checks,
OTLP rejection and other providers' non-Secret skip policy; no ConfigMap feature
is added. Health-check CA/client fields remain policy-local, rejecting explicit
cross-namespace refs before lookup even with a grant. Other outbound CA fields
support Secret/ConfigMap only as enumerated in the manifest. Private contracts
select these finite target sets; a generic custom CA adapter cannot widen them.

The same SecretObjectReference wire type may select different field contracts;
it must not globally assume every use is a Secret, permit aliases in every use,
or reinterpret edgion.io/LinkSys as a Secret. Supported backend targets are
protocol/feature-specific: HTTP AI/delegation, gRPC delegation, strict L4 Service,
and mirrors are not interchangeable. Section 4.5 separates Service-mode
descriptors from the actual resource target; never erase the mode or authorize
the actual lookup using a fictitious kind.

Private target constructors alone are insufficient because strict and custom
fields currently share wire structs. Public adapters must extract the actual
field from its typed owning resource, or accept a sealed non-serialized field
wrapper created by an owner-specific accessor. They cannot accept an arbitrary
SecretObjectReference plus a caller-selected strict/custom profile. Fixed-target
adapters accept the original typed fixed reference or parsed template field,
never name/namespace reconstructed from an unchecked group/kind reference.
This API discipline is enforced by module visibility, field accessor types, and
the frozen consumer audit; it is not a proof of runtime authorization.

Wire schemas remain explicit. Required standard fields use required typed
members without serde defaults and always serialize required group, including
the empty string. Remove skip_serializing_if(String::is_empty) on required group
when removing its default; otherwise a valid empty-group round trip omits a
required field. Optional groups preserve None omission and Some("") presence.
This includes currently defaulted String fields in mirror, Edgion outbound CA,
Webhook Service, and Edgion policy references: migrate them to Option<String>
or an equally presence-aware wire adapter. Optional non-nullable null maps to
omission under the field's schema contract. Do not normalize only a temporary
validation view and then deserialize the unchanged null into a String that
rejects it. Required standard CA/ExtensionRef/Grant groups remain required String
fields with explicit empty serialization; the mixed CA wire split is essential.
Split the currently mixed standard CA wire type
from the Edgion outbound CA wire type, then adapt both into a shared validated CA
target. Required missing/null fields remain invalid after schema-defined null
pruning; typed tests may still construct invalid values and must fail consumer
validation. Do not weaken deny_unknown_fields credential boundaries.

Use a small metadata-only error vocabulary for missing/invalid fields,
unsupported group/kind, and forbidden namespace/scope. Do not include material
bytes, dependency errors, or authorization policy in this pure error type.
Controller adapters map these errors to existing status vocabularies and
partial-validity policies; Gateway adapters preserve each consumer's existing
failure and retained-generation policy. A rejected new target must not be newly
consumed; it does not imply immediate withdrawal of an eligible old generation.
Do not add a generic Ready condition or a new failure-policy knob.

### 4.4 Validated lookup capabilities

The pure target adapter yields a borrowed, immutable SecretTarget or ConfigMapTarget
only when identity and scope are valid for that declaring field. Constructors
and fields are module-private; callers cannot manufacture them from name alone.
Both expose their canonical identity to authorization and key builders.

Bind each capability to the declaring resource identity and field provenance,
not just the target. The typed owning/binding accessor derives source
group/kind/scope from the authoritative consumer context; field wrappers select
the contract statically. An illustrative
pure metadata envelope is:

~~~rust
pub struct SecretLookup<'a> {
    source: ObjectIdentity<'a>,
    target: SecretTarget<'a>,
    field: RefFieldIdentity,
}
~~~

Fields and constructors remain private. Equivalent envelopes exist for other
resource targets. Controller supplies a separate typed dependency owner only to
select the object that receives a requeue. It cannot override the envelope's
authorization source or namespace. Fetch APIs remove independently supplied
owner_group/owner_kind/from_namespace parameters. Readonly/status and direct API
paths use the same source-target envelope, with registration disabled only for
existing readonly derivation.

These lookup envelopes cover reads. ACME account create/patch and certificate
publication use their separately bound operation/write adapters from section
4.2, retaining the original authority and concurrency guards. Shared identity
validation neither adds an EAB grant check nor authorizes a write via a read
envelope. Inventory read and write call sites separately in the frozen plan.

The bound canonical query also feeds authorization_revision capture and async
authority rechecks in the credential manager: LinkSys bootstrap Secret inputs
and EdgionPlugins-to-LinkSys CredentialInjector consumers. Preserve the existing
matching-Grant UID/resourceVersion set, source/input revisions, binding revision
and publication fences when Grants change or are deleted/recreated. Revision
capture uses the same exact matcher as permission decisions, not reconstructed
raw group/kind or independently split unchecked namespace/name strings. Revision
snapshot/lifecycle policy remains Controller-owned. A captured read capability
does not authorize later publication after its authority changes.

Controller fetch_secret/fetch_secret_readonly and typed resolver entry points
accept the bound SecretLookup, not an unchecked SecretObjectReference.
ConfigMap fetching similarly accepts its bound lookup. Reference errors are handled
before lookup and before registration, preserving the current fetch Ok/Pending
contract for valid targets. Material-type/key/decoding failures stay in the
credential material resolver. Update parse, readonly/status, and direct store/API
paths in the same migration unit; do not retain an unchecked parallel entry.

Fixed-type name/namespace callers construct a target through a fixed-target
adapter with their actual owner/scope contract. Template parsing still uses its
existing grammar and provides a fixed Secret identity; it does not gain group
inference or arbitrary defaults.

The implementation plan must demonstrate construction from an actual Gateway
certificate field, an EdgionBackend credential field, and a parsed template
field. Negative API/visibility checks and a caller audit must establish that
strict fields cannot pass through convenience accessors or be rebuilt as fixed
targets. Delegated child source and published root dependency owner remain
distinct; neither is guessed from the other's namespace.

Some reusable ConfigData condition documents contain placeholders rather than
independently resolved references. RequestAccessUrlAllowList items[].conditions
are expanded into EdgionPlugins resolved_candidates before credential binding;
their default namespace and authorization source are the binding EdgionPlugins,
as in condition_tree.rs, not the ConfigData document. A sealed binding accessor
must express this established contract while keeping authored location for
diagnostics and ConfigData dependencies. Do not infer authorization solely from
the file containing the wire value, or transfer resolved Secret bytes from one
consumer's binding into another's. This exception does not change child-local
standard ExtensionRef or delegated terminal-backend authorization.

### 4.5 Backend descriptor, lookup target, and connection strategy

Gateway API requires referenced group/kind to identify a registered resource
type. Current ServiceClusterIp/ServiceExternalName/ServiceEndpointSlice and
empty-group EndpointSlice spellings in standard backendRef.kind do not meet that
requirement. They are existing nonconforming input behavior, not a compliant
extension. Mapping them to a real Service lookup alone does not fix this.

The proposed strict standard adapter accepts a real supported Service or an
exact implemented registered extension target. It rejects pseudo-kinds before
Service authorization, registration, lookup, or dialing, retaining the selected
backend's failure share. ServiceImport remains unsupported/NotImplemented; do
not turn its spelling into a core Service. Real EndpointSlice uses its own
registered group and has no new direct-object support through this task.

Separate three concepts in the API: authored descriptor for diagnostics/slot
selection; actual resource identity for authorization, stores, dependencies and
policy keys; connection strategy for compiled execution. Standard Service refs
resolve to empty-group Service/ns/name. A documented Edgion-owned Service-mode
field, such as existing WebhookServiceRef, may keep its custom mode grammar but
still validates and consumes the actual Service identity. Existing custom
Webhook Service fields have no Controller ReferenceGrant Service authorization
path; preserve their existing source-specific connection/security policy rather
than claim grant enforcement or introduce one in this identity refactor. Material
refs in their TLS configuration remain independently grant-gated. Such custom
mode grammar never propagates into a standard backend adapter.

Preserving route-level mode selection while removing pseudo-kinds requires a
separately approved Edgion annotation/policy field. The preferred direction is
standard Service references plus a typed Edgion connection-strategy carrier;
existing per-route/per-slot distinctions must not silently become one global
per-Service setting. This specification does not invent or approve that carrier.
Routing implementation is gated on an explicit product choice: approve its
exact surface/default/conflict/requeue contract, or approve rejection of the
nonconforming old inputs. No runtime behavior changes in this design phase, and
there is no legacy pseudo-kind rewrite or compatibility shim.

Selectors preserve canonical authored descriptor dimensions, including invalid
input distinctions. Policy keys use the actual supported target, not a pseudo
kind or a core sentinel. Compile supported target plus strategy once; peer
selection consumes it and validates mutable context binding instead of parsing
raw kind. Include pg_upstream_peer and backends/mod.rs gRPC input in the frozen
migration plan. Neither path may read a core Service for an unsupported identity.

### 4.6 Narrow source-schema validation view

Admin create/update/validate currently validates raw Gateway API JSON before
typed adapters (api/common.rs::validate_gateway_api_raw and validate_handlers.rs).
Plain JSON Schema rejects optional null even where Kubernetes would prune it.
Option deserialization later cannot repair this earlier disagreement.

Generate a temporary reference-validation view before those existing schema
checks and any offline source-adapter pre-schema check. Select exact supported
source GVK first; prune non-nullable null only at inventoried reference paths,
and apply a declared reference default only when the existing source schema
requires materialization to validate. Required-without-default stays missing
and invalid. Retain the authored document and all other schema checks. Do not
prune arbitrary fields, make missing required group optional, add aliases to
standard fields, install watcher-wide admission, or weaken existing transform
and version checks. This cold-path view and typed adapters derive reference
semantics from one static field-contract inventory, not two handwritten tables.

Select the source GVK together with its existing code-owned pinned schema
registration profile for offline/Admin validation. GVK alone does not identify a
bundle/channel contract. Freeze scoped reference fragments for both v1.6.2
channels, including enclosing list/union constraints and reference-relevant CEL.
Standard parentRefs uniqueness uses sectionName; Experimental includes port.
Canonical identity must not deduplicate authored refs or erase port/section
differences. Preserve applicable registered-source rules; do not choose an
intersection or union of channels, guess a channel from v1, or add an operator
channel field. Kubernetes admission remains owned by the installed CRDs; unknown
installed bundle/channel remains unverified under existing SupportedVersion
behavior, not a new compatibility claim or discovery feature.

The temporary null/default view establishes only its inventoried leaf semantics.
Current generic SchemaValidator runs OpenAPI, not Kubernetes CEL. Inventory
reference-list bounds/topology, core Service port requirements, rule/backend
ExtensionRef discriminator/payload rules, parent section/port grouping, policy
target section uniqueness and CA-ref/wellKnownCA exclusion. Keep canonical
identity separate from owning list/union validation and status policy. Reuse the
existing scoped generated-contract infrastructure where applicable; review any
reference-specific extension or explicitly retain an admission-parity gap. No
whole-resource CEL/admission engine or new watcher-wide validation is introduced.

### 4.7 Approved cluster-owned material authority

EdgionGatewayConfig is cluster-scoped. Standard ReferenceGrant has no valid
cluster source namespace representation. The baseline's default/empty source
namespace fallbacks were inconsistent observed contracts, not standard defaults.

The explicitly approved contract is a narrowly enumerated Edgion administrative
authority for EdgionGatewayConfig.spec.outboundTls.validation.caCertificateRefs[]
and .clientCertificateRef only. The source stays Cluster, material namespace
must be explicit/nonempty, and no standard ReferenceGrant is required for these
two fields. Possession of EGC write authority authorizes selection
of named material in explicit namespaces. A cluster-scoped object does not by
itself prove its writer has cluster-admin rights; this delegation of authority
was approved in 00-context and is documented in RBAC/Admin ownership guidance.

Keep material lifecycle dependencies and clear-before-add behavior. Register
grant dependencies only on grant-gated paths; do not register phantom grants
for this administrative selection. It does not apply to GatewayClass,
namespaced resources, global plugin attachments, or arbitrary custom refs.
The owning accessor privately mints authority only for these actual EGC fields;
ordinary tokens cannot be upgraded and no caller-selectable bypass flag exists.
Malformed owner identity is rejected even without material refs. Canonical
cluster ownership drives clear-before-add cleanup; target namespaces never default.

## 5. End-to-end data flow and invariants

~~~text
authored wire fields + declaring field contract + owner/origin context
    -> pure validated canonical target (or reference error)
    -> register valid material/cross-namespace dependencies
    -> exact authorization under existing resource policy
    -> matching store/material lookup
    -> publish bound resolved state and denial/status
    -> Gateway independently validates and compiles canonical target
    -> consumer applies its existing candidate-failure/retention policy
    -> newly selected supported target may be consumed
~~~

1. Retain raw authored optional fields for serialization and diagnostics; do not
   fill every None with an allocated String. Effective identity is borrowed.
2. At reconcile entry, clear old derived material, denials, and dependency sets
   using existing clear-before-add boundaries. Invalid identity registers no
   edges pretending to be a same-name core object. A valid missing or denied
   target still registers dependencies before the grant/store read.
3. Use the existing read_after_registering_cross_ns_dependency ordering helper;
   resource/grant events continue to requeue the single per-kind worker. No new
   ref registry, second queue, polling, timers, or handler API calls.
4. Identity classification does not decide authorization. Gateway attachment uses
   parent/allowedRoutes; route delegation uses existing reverse parent attachment
   consent; ReferenceGrant gates the existing cross-namespace object consumers;
   ConfigData visibility Cluster retains its established exemption. The proposed
   exact EGC administrative material authority in section 4.7 is a separate
   approval gate, not a standard ReferenceGrant source.
5. Delegated filters are local to the declaring child. Preserve controller-owned
   resolved_namespace and reject operator attempts to provide it according to
   existing ingestion rules. Endpoint/mirror authorization uses the child's
   declaring identity, while dependency registration tracks the published root.
6. Grant denial transport and TLS material source binding remain explicit.
   Gateway cannot validate a newly built reference and attach material belonging
   to a different canonical source. Check UID/generation/revision only where the
   existing binding protocol supplies and requires it; a name reference alone
   does not identify a Secret generation. Preserve the accepted EdgionTls
   last-known-good policy: an invalid same-key update/full snapshot may retain
   the entire preceding validated runtime, including old refs/material and TLS
   identity. Never mix a new reference with old material, and do not add an
   equal-UID retention gate. Explicit-add, absent-key, full-empty, and invalid
   attachment behavior remain those of the existing TLS store.
7. Invalid selected backend slots retain their weights and fail closed; they are
   never dropped to redistribute traffic. HTTP, gRPC, and every L4 final dial
   path consume a compiled supported target, not an independently parsed raw kind.
8. Native mirror failure still skips only that mirror. ExtensionRef preserves
   discriminator/payload checks, rule/backend execution scope, and preflight gates.
9. Canonical slot identity stores empty group as empty, never the sentinel core.
   Build occurrence tuples and selector comparisons from the canonical authored
   descriptor; policy and lookup keys come from the compiled actual target as
   specified in section 4.5. An unsupported explicit core descriptor cannot merge
   with an empty-group slot.
10. Preserve secret redaction and schemars visibility. Derived runtime identities
    do not become operator fields. Gateway hot paths gain no allocation, locks,
    group parsing, logging, or unbounded registry operations.
11. Preserve EdgionPlugins DSL/native-WAF whole-generation last-known-good policy
    as well as TLS retention. Bundle deletion/grant revocation may invalidate a
    new candidate while an eligible previous compiled generation stays active.
    Current class policy, enabled JWE/cross-namespace mirror exclusions, explicit
    removal/full-empty, and sealed-failure behavior remain enforced. New reference
    validation does not imply immediate withdrawal of all retained programs.
12. Inline custom outbound Service defaults are fixed at plugin compilation
    using the actual EdgionPlugins source. The proposed owner-local correction
    for ForwardAuth/Guardrail/AiGuard replaces their current default-namespace
    fallback only after explicit implementation-impact review. Pass the compiled
    bound target to ManagedOutboundClient; do not guess the source on requests.
    ForwardAuth local TLS material wiring is a separately identified correction
    in the material unit, with parse/status/dependency/runtime tests before any
    claim of support. Neither change adds an operator field or a grant policy.
13. Preserve LinkSys provider-owned lifecycle. Ordinary same-type NotReady,
    fingerprint/build failure and handover retry can retain a previously
    published persistent client. Global inherited-TLS forced generations retire
    the old persistent publication before trying the replacement, including a
    NotReady candidate; forced Webhook rebuild also withdraws old publication.
    Webhook/HttpDns keep their own preparation and replacement policies.
    Explicit removal/type change withdraws current lookup and signals retirement;
    caller-held handles retain existing semantics. No cross-provider barrier or
    immediate global handle-revocation guarantee is introduced by reference
    validation or ordinary grant loss.
14. Dedicated Gateway Service binding preserves namespace/name, UID and
    deterministic Service naming as one protocol. Fixed log-output refs preserve
    required explicit namespace/name, sink-kind checks and log ownership. ACME
    generated/write identities preserve source/output namespace distinctions,
    write authority and attempt/target fencing; no read capability or ordinary
    ReferenceGrant policy is inferred from the identity parser.
15. Standard Grant-required Gateway material, route backendRefs and native
    mirrors must revoke granted access when authorization is lost. After the
    denied generation is published, new requests/connections cannot consume the
    revoked target or obtain same-name material through a fallback store. Custom
    EdgionTls/DSL-WAF/LinkSys retention is confined to its exact existing contracts
    and cannot justify retention for a standard field. Existing in-flight
    snapshots and established-session behavior are separate; no instantaneous
    cancellation guarantee is claimed.
16. Grant permission and credential-manager authorization_revision use the same
    bound canonical source/target and exact matcher. Same-name Grant recreation,
    changed from/to terms and multiple surviving Grants preserve the existing
    revision and publication fencing semantics. Revision-bearing protocols do
    not impose UID requirements on ordinary reference identity.
17. Generated CredentialSource identity retains consumer-local Secret output,
    independently bound source LinkSys and parse/readonly binding/expiry parity.
    Manager-side exact ownership/revisions, CAS cleanup/publication and leadership
    rules remain separate; sharing identity does not add handler I/O or fences.
    Persist-off retains one active source-UID entry and existing mode-transition
    behavior. No general ownerReferences lookup or generated-name authority.
18. EndpointSlice Service label association and DNS outputs keep their distinct
    field-specific fallback, ownership, naming, last-good and GC policies. Valid
    relabel/replacement rebuilds both affected pools. The namespace selector in
    allowedRoutes remains label-selection policy, not an object/Grant capability.

## 6. Migration units and scope

These are design units, not permission for partially deploying inconsistent
contracts. Keep one serial implementation lane. A unit that changes field
semantics must update its schema, Controller, Gateway, status/read-only paths,
tests, docs, and knowledge together before it is complete.

| Unit | Scope and principal consumers | Completion boundary |
| --- | --- | --- |
| 1: pure module | Resources refs, parent identities, ExtensionRef validator, standard/custom wire adapters | Contract matrix and zero-allocation design reviewed; no new alias enabled accidentally |
| 2: material identity | Controller credential pipeline, CA helper, manifest-listed plugin/condition/LinkSys/AI/global TLS consumers, credential-manager authority revisions/generated bindings, ACME direct API, EdgionTls, Gateway TLS | No unchecked lookup or readonly bypass within the frozen field scope; source binding, published-retention boundaries and stale cleanup proven |
| 3: routing and policy | HTTP/gRPC/L4 classification/selection, pg_upstream_peer, mirrors, filters/delegation, policy targets, AI slots, scoped conf_sync, publisher | Actual Service lookup/dial obeys compiled identity; weighted invalid shares and local origin preserved |
| 4: fixed refs and selectors | GatewayClassName, ConfigData, exact LinkSys/DNS/string/template and log-output refs, instance bindings, ACME/CredentialSource/DNS output identities, EndpointSlice Service association, global plugin refs, RegionRoute refs, allowedRoutes, parametersRef | Reuse typed identities without changing selector/alias/locality/visibility, UID protocol, output authority or write fencing |

Every unit that touches raw input also covers Admin create/update/validate and
existing offline source-adapter pre-schema boundaries in section 4.6. Material
and routing units remain gated on their explicit product decisions in sections
4.5/4.7. A no-semantic-change foundation unit cannot claim those gaps closed.

During implementation planning freeze exact file/field allow-lists for each unit
and audit all callers with rg. Source changes will span Resources, Controller,
Gateway, custom CRDs, and relevant tests/docs; this is a shared-core design, not
a single helper patch. Standard vendored CRDs are authority and must not be edited
to make core aliases or missing required fields pass. Custom CRDs must match the
selected convenience contracts; property-presence guards alone do not establish
required/default/enum/nullability parity.

Unsupported Gateway infrastructure parameters, ListenerSet features, native gRPC
mirror support, and experimental ExternalAuth do not become implemented through
this inventory. No MCP/A2A, ACME DNS-01, Gateway rustls, general schema discovery,
new arbitrary resource resolver, or new remote lookup fallback is introduced.
The field manifest explicitly classifies service-owned conf_sync PKI/Lease
bootstrap, generic metadata and synthetic non-resource placeholders as outside
this migration. Its no-unchecked-lookup requirement applies to frozen reference
consumers, not every service-owned Kubernetes API operation in the repository.

## 7. Verification and acceptance

### 7.1 Pure and schema tests

- Per field contract: omitted, explicit empty, explicit core, supported custom
  group, unknown group, case variants, whitespace, null, wrong/empty/missing kind,
  missing/empty name, namespace scope, section, and port.
- Scoped source-GVK/bundle/channel reference contract includes list bounds and
  reference-relevant CEL. Test same-parent section/port combinations separately
  for Standard/Experimental; Service missing/invalid port; ExtensionRef union
  both directions; policy target section uniqueness; CA-ref/wellKnownCA exclusion.
  Do not collapse distinct refs after defaulting. Null-view or OpenAPI success
  alone does not prove owning-list/CEL admission parity.
- Standard optional omission and API-server-defaulted representation yield the
  same effective identity. Standard required missing/null-without-default inputs
  remain invalid after schema-defined null pruning across YAML/JSON/FS/etcd/Admin.
  Optional non-nullable null prunes to omission/default; nullable fields follow
  their own contracts.
- Custom convenience acceptance is tested only for the exact published field
  manifest; the same shared wire struct in a strict field rejects the alias.
- ReferenceGrant from/to exact comparison, both mixed core/empty directions,
  wildcard name, OR-list entries, and empty/default namespace edge cases.
  Test exact namespaces, invalid explicit empty to.name, same-Grant from/to
  conjunction, OR across Grants, and rejection of cross-Grant term composition.
- Split standard/custom CA schemas: required fields, namespace absence/allowed
  scope, defaults, custom enum values, and serde/conf_sync round trips. Required
  empty groups remain serialized, including LocalObjectReference.
- Per-prefix TLS target matrix: LinkSys Secret-only CA and Secret type validation,
  OTLP unsupported-kind rejection versus other providers' skip policy, plugin
  Secret/ConfigMap CA, and policy-local health CA/client references. Cross-namespace
  health material remains rejected even with a matching grant.
- Keep unit-test bodies in the mandated tests/unit mirror layout and preserve
  existing meaningful test identities and omission fixtures.

### 7.2 Controller and Gateway tests

- Invalid identity causes zero wrong-type material/store/API accesses and zero
  fabricated material dependencies, even under lenient admission and readonly
  status derivation. Valid-but-missing/denied references retain required edges.
- Controller-global Grant policy on/off preserves each current consumer's scope
  without new credential bypasses. Invalid identities remain invalid while off;
  target/dependency registration stays correct. Conformance runs require on.
- Delayed credential acquisition/publication rechecks canonical authority after
  matching Grant delete/recreate, from/to updates and removal of one of several
  matching Grants. Cover LinkSys Secret inputs, EdgionPlugins-to-LinkSys binding
  and custom core/empty equivalence without changing publication ownership.
- EdgionBackend, OAuth2 active/previous refs, plugin refs, OTLP, CA, and direct
  ACME APIs share identity validation behavior within their field contracts;
  reconciled candidate material/dependencies clear before replacement. Published
  old runtime generations obey each consumer's separate retention policy.
- Cluster-scoped owner with a namespaced target requires the field's explicit
  namespace; fixed global plugin refs retain their documented default namespace.
- gRPC malformed direct transport cannot reach same-name Service lookup/dial.
  HTTP/gRPC/L4 invalid weighted targets retain rejection shares.
- Standard pseudo-kinds are rejected without reading same-name Service. Approved
  custom mode descriptors stay distinct while real Service grants/policies apply;
  pseudo-kind grants cannot authorize actual Service reads.
  ServiceImport remains unimplemented rather than redirected to core Service.
- ExtensionRef union, child-local origin, rule/backend scope, unsupported group,
  CORS preflight, and raw-child delegation regression guards remain effective.
- Delegation named/wildcard selectors preserve enumeration/reverse consent and
  dependencies; no fabricated '*' object identity enters grants/stores/policies.
- Inline Service omitted namespace uses the proposed plugin owner context after
  approval, while explicit namespace and LinkSys defaults stay correct. Test
  plugin namespaces other than default; invalid group never falls back to Service.
- ForwardAuth local CA/client fields have explicit wiring coverage for valid,
  missing, denied, invalid, update and removal, or remain explicitly unsupported
  pending that correction; shared wire presence is not a support claim.
- Parent and status identities preserve group/kind/namespace, section/port, and
  controller ownership, including same-name Gateway/Route collision cases.
- TLS resolved outcomes remain bound to exact canonical source identities;
  custom alias and empty spelling produce one identity without stale fallback.
- EdgionTls last-known-good retains the whole predecessor on invalid same-key
  update/full-set; explicit add does not inherit; absent/full-empty removes;
  invalid attachments revoke admission; valid recovery replaces normally.
- EdgionPlugins DSL/WAF retention, policy/JWE/mirror exclusions, absent/full-empty
  clearing, and valid replacement retain their existing revocation boundaries.
- LinkSys ordinary NotReady/invalid candidate retains old eligible persistent
  publication under existing handover rules; forced global TLS rebuild retires
  old publication before a failed/unready replacement. Cover Webhook force,
  HttpDns policy, removal/type change, valid recovery and old-handle semantics
  separately; no universal immediate grant-revocation claim.
- Dedicated Gateway Service annotation and process-scope bindings preserve exact
  namespace/name plus supplied UID, mismatches and same-name replacement guards;
  Python/Rust producer-consumer fixtures agree on derived names and serialization.
- Log output/fallback refs preserve explicit namespace/name validation and actual
  provider-kind lookup. Do not merge output::LinkSysRef with DSL alias semantics.
- ACME Secret destination and generated EdgionTls names preserve omission and
  explicit namespace behavior, including different Secret/Acme namespaces.
  Read/create/patch and publication tests retain Lease/source cancellation,
  target UID/resourceVersion preconditions, forced SSA and concurrent account
  Secret merge-patch guards; ordinary SecretLookup is not a write capability.
- Generated credentials cover cross-namespace LinkSys sources with consumer-local
  output, source/consumer same-name UID replacement, spoofed/conflicting Secrets,
  stale publication/cleanup, expiry and persist-mode transitions. Parse/readonly
  keep their existing matching-owner, UID/source-generation, binding-revision and
  expiry checks; manager operations separately test stale leadership/publication,
  exact ownership, CAS and UID/resourceVersion cleanup. Each operation retains
  its applicable checks rather than acquiring identical reader/writer fences.
- EndpointSlice association covers A-to-B relabel, remove-plus-same-key-add,
  absent Service label/namespace policies and old/new pool retirement. DNS output
  covers IPv4/IPv6 naming, Service namespace/UID, last-good resolution failure,
  forced SSA and garbage collection without new backend target support.
- Every distinct fixed LinkSys field and annotation has representative grammar,
  non-default namespace, wrong-provider and failure-policy coverage. Controller
  DNS tests distinguish Kubernetes API lookup from kube-less system fallback;
  DSL aliases and Redis counter namespaces remain separate dimensions. Preserve
  Namespace-label selector events and actual route/listener plugin stages.
  Gateway HTTP-plugin list fixtures distinguish comma/space/tab/LF separators
  from embedded CR/form-feed/vertical-tab, which do not split references.
- Admin create/update/validate use the same reference validation view before raw
  schema checks; optional-null/default/required-null cases agree with the field
  contract without bypassing unrelated schema/transform validation.
- Approved cluster material authority covers only the two exact EGC fields,
  requires explicit namespace, uses true Cluster source, and never widens a
   namespaced grant-gated field. No fake default/empty source namespace remains.
- Logical slot occurrence/dedup and policy-key equality use canonical groups;
  dynamic backend selectors retain wildcard namespace semantics.

### 7.3 Integration and mode parity

Retain existing shared Gateway/Controller topology and existing fixtures. Add
representative dynamic valid-omitted -> invalid explicit standard core -> valid
omitted recovery, and separate custom empty/core equivalence phases. Exercise
standard Grant deletion/recreation as allowed -> revoked/no new target use ->
allowed recovery for Gateway serving/client/CA material, backendRefs and native
mirrors, preserving valid siblings and invalid weighted shares. Custom retention
cases use their separately named lifecycle contracts, with event-driven waits,
not sleeps or a second topology. Updating Grant from/to terms must revalidate
dependents of both old and new terms, including compressed delete/recreate.

Compare accepted raw FS/etcd/Admin data with typed Kubernetes-defaulted values
and Gateway conf_sync consumers. A live API-server case is needed to claim
admission required/default/nullability parity; pure fixtures and schema validators
are narrower evidence. Do not claim all production modes were executed when only
the FS shared topology ran.

Record implementation revision/image, conformance-suite version, actual installed
CRD bundle/version/channel and artifact/digest/annotations, selected source GVKs,
profiles/features, base manifests/topology, filters/skips and raw reports for each
live acceptance run. The current harness sets AllowCRDsMismatch=true; verify
installed schemas independently rather than infer them from the Go dependency.
Run affected reference/status/dynamic cases after implementation, separately from
required/null/CEL admission checks. Targeted success is not a full-profile pass.

Historical reports are baselines only. The saved 2026-09-24 Standard v1.6.2 run
records HTTP Core 36/37 (HTTPRouteMultipleGateways failed), HTTP Extended 26/30
(BackendTLSPolicy, InvalidCACertificateRef, InvalidKind and SANValidation cases
failed), GRPC Core 15/15 and TLS Core 20/20 without an Extended claim. The
Experimental 2026-09-23 HTTP 37/37 plus 30/30 result does not replace Standard
evidence. Preserve those known failures and do not attribute them to this design
or claim they are fixed without fresh evidence. Supported feature/source-version
declarations do not change merely because the shared reference module exists.

### 7.4 Project gates for implementation

Run the AGENTS.md format/workspace-check/workspace-clippy/doc/SSA/metrics/Pingora/
Gateway-transform/unit-layout gates, focused affected tests, required CRD contract
checks, and relevant Gateway no-tracing/English-only checks. Full cargo test --all
remains explicit-user-only. Independent final code/knowledge review is required
before closing the source issue. Spec-only validation consists of document
structure, sources/links, English-only content, and independent design review;
it is not runtime verification.

## 8. Documentation, knowledge, and decisions

Implementation must publish a concise reference-default table and custom
convenience field manifest in relevant English/Chinese user docs. Clarify core
as a group concept versus the literal string core. Explain why ExtensionRef still
requires edgion.io/EdgionPlugins and why ReferenceGrant targets use empty group.

Update architecture/unification guidance only when the module is implemented.
Rewrite the historical shared-service predicate page to separate past dedup
evidence from the new contract; update owning routing/material/authorization
guards by knowledge-routing rules rather than creating a closed-finding ledger.
Keep the original investigation open until implementation/review/check closure,
or an explicit independently reviewed no-code disposition.

### Design review record

| Review | Model/independence | Verdict and decisive outcome |
| --- | --- | --- |
| Cross-review: Controller | Existing Sol inventory agent; did not author spec | Issues Found: sealed field accessors, source-target authority binding, and TLS whole-generation retention needed explicit contracts; amended sections 4.3/4.4/5/7 |
| Cross-review: Gateway/standard | Separate existing Sol inventory agent; did not author spec | Issues Found: Kubernetes null pruning, Service-mode versus actual target, and required empty-group serialization; amended sections 3.1/4.3/4.5/7 |
| Astra round 1 | Fresh gpt-6-astra reviewer, read-only | Issues Found: pseudo-kind conversion still violated registered-target contract; EGC cluster source authority undefined; Admin raw-schema null boundary omitted. Amended sections 4.5/4.6/4.7 and explicit product gates; added plugin/WAF retention |
| Astra round 2 | Fresh gpt-6-astra reviewer, read-only | Initially identified exact ExtProc path, missing GatewayClassName, custom Service authorization/defaults, ForwardAuth TLS wiring, delegation selectors, and optional String decoding gaps. Latest revision LGTM after amendments; product gates remain unapproved |
| Astra round 3 | Fresh gpt-6-astra reviewer, read-only | Issues Found: per-prefix CA/locality restrictions, missing fixed/log/output identities, and LinkSys retained-generation boundaries. Amended sections 2/3/4/5/6/7 and field manifest; latest full-document re-review LGTM, no remaining design blockers; product gates remain unapproved |

Initial Astra thread-capacity failures were not reviews and the Sol cross-reviews
are not labeled Astra. Record final re-review outcomes on the latest documents;
design review does not establish runtime correctness or approve product gates.

The subsequent user-requested two-round checks in three directions are recorded
in [03-specialized-checks.md](03-specialized-checks.md). Round 1 found nine scoped
design/coverage/acceptance omissions; the spec and manifest have been amended.
Round 2 exchanged reviewer directions, corrected two further precision defects
and concluded final LGTM for Grant consistency, reference coverage and conformance
impact. No remaining design blocker was found within those audited scopes.

### Spec-only verification

- Targeted checks passed for the new documents' local links, heading structure,
  whitespace/newlines, and forbidden writing-script ranges. Git diff --check
  passed for tracked changes; targeted checks also cover these new untracked docs.
  Final checked inventory: five task documents and 41 local links.
- Repository validate_agent_docs.py failed on the unchanged AI-routing knowledge
  page's stale OpenAI request.rs source reference, recorded in 05-issues.md and
  confirmed present at HEAD. No repository-wide document-pass claim is made.
- No production code, custom/standard CRDs, user docs, or review knowledge were
  modified. No Cargo/integration/live Kubernetes test or commit/push was performed.

### Historical implementation confirmation gates

The user selected these decisions in 00-context. This list records the design-phase
gates and does not request another approval or block the authorized implementation.

- Approve the exact Edgion core-convenience field manifest; retaining strict
  ACME/EdgionTls and strict standard references is the recommended boundary.
- Approve the internal API migration and standard/custom CA wire split.
- Approve the precise migration of nonconforming route mode spellings, or their
  strict rejection, before changing routing behavior (section 4.5).
- Approve the exact EGC administrative material authority and its write-permission
  implications before changing cluster material resolution (section 4.7).
- Include inline Service namespace-default correction and ForwardAuth local TLS
  material wiring in the reviewed implementation impact before enabling them.
- Preserve EndpointSlice malformed-input fallback differences, or explicitly
  approve a behavior correction before harmonizing them.
- Freeze migration units and exact supporting edits after plan review.

## 9. Sources and relevant original knowledge

- [Gateway API object references v1.6.2](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/apis/v1/object_reference_types.go)
- [Parent/reference shared types v1.6.2](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/apis/v1/shared_types.go)
- [ReferenceGrant v1.6.2](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/apis/v1/referencegrant_types.go)
- [Policy targets v1.6.2](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/apis/v1/policy_types.go)
- [Kubernetes CRD null pruning and defaulting](https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/#defaulting-and-nullable)
- [Vendored standard CRDs](../../../config/crd/gateway-api/gateway-api-standard-v1.6.2.yaml)
- [Envoy Gateway target validation](https://github.com/envoyproxy/gateway/blob/65dd9815ce3398c5bc0b1158a3fa6edced6c2aa2/internal/gatewayapi/validate.go)
- [Envoy Gateway local schema defaulting](https://github.com/envoyproxy/gateway/blob/65dd9815ce3398c5bc0b1158a3fa6edced6c2aa2/internal/gatewayapi/resource/load.go)
- [Kong backend GroupKind implementation](https://github.com/Kong/kubernetes-ingress-controller/blob/b6da030fde88a6a64f6ba42696ae666001a2e049/internal/util/k8s.go)
- [Kong exact ReferenceGrant matcher](https://github.com/Kong/kubernetes-ingress-controller/blob/b6da030fde88a6a64f6ba42696ae666001a2e049/internal/gatewayapi/references.go)
- [ExtensionRef union and target guard](../../../skills/04-review/routing/regression-guards/gateway-api-extensionref-validation-fails-closed.md)
- [Delegated ExtensionRef locality](../../../skills/04-review/routing/regression-guards/delegated-extensionref-remains-child-local.md)
- [Delegation parent identity](../../../skills/04-review/routing/regression-guards/delegation-parentref-gateway-name-collision.md)
- [Parent status ownership](../../../skills/04-review/status/regression-guards/canonical-parent-status-ownership.md)
- [Dependency before authorization](../../../skills/04-review/handler-requeue/regression-guards/reference-grant-dependency-before-authorization.md)
- [Controller-owned Grant validation policy](../../../skills/04-review/resources/intentional-design/reference-grant-validation-controller-scoped.md)
- [ACME strict Secret identity](../../../skills/04-review/acme/regression-guards/secret-reference-target-identity.md)
- [ACME side-effect fencing and recovery](../../../skills/04-review/acme/regression-guards/acme-scheduler-fencing-recovery.md)
- [LinkSys provider-owned lifecycle and removal](../../../skills/04-review/link-sys/intentional-design/provider-owned-runtime-and-simple-removal.md)
- [LinkSys TLS target support](../../../skills/01-architecture/05-resources/18-link-sys.md)
- [Health-check policy-local TLS](../../../skills/01-architecture/05-resources/20-edgion-backend-traffic-policy.md)
- [CredentialSource active-memory identity](../../../skills/04-review/link-sys/regression-guards/credential-source-single-active-memory.md)
- [EndpointSlice association and pool replacement](../../../skills/04-review/backends/regression-guards/endpoint-slice-ownership-transitions.md)
- [Gateway API schema validation limits](../../../skills/08-kubernetes/01-gateway-api/07-version-support/03-validation-current-state.md)
- [Actual conformance report boundaries](../../../edgion-tests/conformance/reports/README.md)
- [Standard HTTP/GRPC v1.6.2 baseline](../../../edgion-tests/conformance/reports/gateway-http-grpc-v1.6.2-standard-kind-2026-09-24.yaml)
- [Standard TLS v1.6.2 baseline](../../../edgion-tests/conformance/reports/gateway-tls-v1.6.2-standard-kind-2026-09-24.yaml)
- [OTLP consumption and transport guard](../../../skills/04-review/link-sys/regression-guards/otlp-provider-ownership-and-egress.md)
- [Historical Service predicate dedup](../../../skills/04-review/routing/regression-guards/shared-service-backend-ref-predicate.md)
- [Existing schema validation placement](../../../skills/04-review/resources/intentional-design/schema-validator-non-k8s-asymmetry.md)
- [EdgionTls same-key last-known-good policy](../../../skills/04-review/tls/intentional-design/last-known-good-follows-resource-key.md)
- [EdgionPlugins DSL/WAF whole-generation retention](../../../skills/04-review/plugin-runtime/intentional-design/edgionplugins-key-scoped-last-known-good.md)
- [Current EGC cluster-material architecture](../../../skills/01-architecture/05-resources/03-edgion-gateway-config.md)

## EdgionPlugins reference-target ambiguity

EdgionPlugins reference lookup distinguishes semantic target identity from exact
raw source ownership. A default/name target may resolve raw default/name or raw
name (omitted namespace), but never an explicit empty namespace /name. Other
namespaces require the exact qualified key. Zero sources is missing; one is
unique; both are ambiguous even if content or UID agrees. This is specific to
EdgionPlugins and grants no authorization or aliases for other resource kinds.

Ambiguity means no uniquely resolvable source. New scopes publish a sealed
non-retainable denial without candidate policy, AI or log declarations; Bypass
cannot suppress it. Route ambiguity diagnostics use ResolvedRefs=False without dropping
rules or backend slots. Ordinary missing-reference and unique-source scope
policies remain unchanged.

A collision clears the semantic selection and its retained candidate. After
collision removal, only the survivor's current generation may activate: an
invalid survivor fails closed rather than reviving a pre-collision executable.
Normal same-raw-key DSL/WAF retention, including UID changes and full relists,
keeps its existing policy/material exclusions. No equal-UID retention gate is
introduced. Exact raw inventory, deletion, credential ownership and entry/rate
scopes remain unchanged; retained execution never crosses raw owners. Existing
in-flight captured scopes and independent-kind publication boundaries remain.

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

### Condition IpList owning references (R5c1)

`PreparedConditionReferences` owns both ConditionSet IP leaves and RAP direct
IpAccessMatch fields after sidecar expansion. Fixed ConfigData identity validation
precedes lookup, Grant queries and dependency registration. Omitted target namespace
uses the actual EP's effective namespace (default when omitted); reverse dependencies
retain `Kind:raw-resource-key`. Invalid identities have no recovery edges; valid
missing/denied targets retain them. Missing/wrong-type targets remain unavailable;
present right-type targets are permission-checked before the enable flag.

IP activity preserves enabled-only entry/body/dye roots and request-stage RR/RAP,
independently of credential supported-stage gating. Ordinary IP consumers remain
unresolved on reference failure; RAP IP consumers retain partial-good unions and
bounded warnings. The owning visitor binds warning application to the effective
RAP entry, preserves URL-before-IP order, and traverses profile keys deterministically.
R5c2 binds standalone HTTP, connection, TLS-route and every GCIR profile's
allowRefs/denyRefs through `PreparedIpListReferences`. The shared Controller resolver
accepts only actual edgion.io EP/ESP sources and fixed edgion.io/EdgionConfigData
IpList targets before reads or Grant queries. Source binding failures and target
slot failures are distinct: a reference-bearing invalid owner cannot construct a
runtime; an invalid target contributes no external set, while authored allow intent,
inline rules and valid siblings remain effective. Deny-only invalid external sets
retain the existing drop behavior. Existing blank-name structural validation remains
unchanged. Controller status reports invalid identities and preserves dependencies
for valid missing or denied targets, including disabled standalone entries.

Gateway constructors borrow the actual immutable owner entry at compilation;
production has no detached config/namespace constructor. Runtime disabled-entry and
policy handling, raw rate/cache identity and one snapshot per decision remain unchanged.
Old views retain their target keys while subsequent decisions observe live IpList
publication. GCIR Selector, RegionRoute, WAF and other ConfigData roles are outside
this standalone IpList migration.

## WAF owning reference identity

The finite WAF fields use synchronous actual-owner loans: EP owns policyRef and activeProfileRef;
the captured WafPolicy ConfigData owns bundleRefs. Validate owner first and each target before
lookup, dependency registration or Grant query. Owner namespace omission supplies semantic `default`
without rewriting raw owner/store keys or aliasing bare ConfigData targets. Capture still holds one
map read guard; policy limits precede child copies, and only selected accepted bundles survive in
resolved material. Existing blank/slash shape failures remain structural. Newly invalid optional
bundle slots are skipped with bounded warnings; required slots invalidate their profile and an invalid
default invalidates the candidate. Selector identity failure retains existing default fallback.
Gateway checks actual EP policy identity at its cold cache boundary and selected transport bundle
identity in the compiler; transported policy/namespace values never become resource capabilities.
Existing current policy, LKG, scoped source-copy and in-flight pinning rules remain unchanged.

## R5e finite plugin ConfigData fields

RAP URL document, RAP/Canary/GCIR Selector and RegionRoute override references use actual owning-entry loans with fixed edgion.io/EdgionConfigData roles. Owner failure is distinct from target unavailability. Invalid identities produce no external lookup, Grant query or dependency edge; valid missing/denied references retain recovery edges. Target namespace omission uses the owner's effective default namespace independently of raw owner keys.

RAP configured Selector failure remains a denying RAP filter under existing condition/policy placement. Canary/GCIR Selector failures and Region override failures retain validated base fallback. URL imports skip invalid/denied/unavailable units, preserve inline and permitted siblings, retain the 64-warning bound and authorize copied conditions as the EP. Readonly URL status consumes parse-time warnings; Selector/Region status recomputes current reference permission. Actual owning guards precede cold cache hits. Runtime immutable snapshot and pin contracts remain unchanged. R6-R8 and live traffic verification remain open.

### R6a Controller DNS ownership boundary

Service and Gateway DNS annotation decisions borrow the actual owner and share
the finite explicit namespace/name grammar with process DNS configuration.
Controller handlers register valid cross-namespace raw-owner recovery edges
before checking the current typed Grant query. The raw Kubernetes DNS producer
uses that same permission decision before acquiring LinkSys. A denied annotation
falls through to same-layer inline servers and then the administrative default;
the default retains its independent authority even for the same target key.
Required generated EndpointSlice families must match the current Service UID,
name, namespace, family and service label before a matching successful content
fingerprint may skip forced SSA. This does not add an async publication fence
or delete obsolete families for a still-desired Service.

### R6b Gateway DNS cold consumers

Service/Gateway DNS annotations now enter cold candidate construction through
actual owner loans; changed Service projections publish candidates with geometry,
and captured attempts borrow that same generation. EGC DNS selects its raw-key
winner before validating its actual cluster owner and explicit typed LinkSys
object. Invalid configured EGC references select System, without runner-up or
inline fallback; inline-only settings and TTL remain independent. Common has one
borrowed-layer precedence implementation. Provider availability remains live.
This closes only these DNS consumer rows, not other R6 references, generated-output
authority, R7/R8, documentation cleanup or real Gateway API conformance.
