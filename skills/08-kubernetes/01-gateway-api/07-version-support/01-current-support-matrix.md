# Current Gateway API version support matrix

This matrix reflects the code-owned `gateway_api_support::SOURCES` inventory
and a static audit of current ingress on 2026-09-24. It separates declared
product support from compatibility code and versions merely exposed by the
vendored Gateway API v1.6.2 CRDs. Revalidate it after any type, watcher, CRD, or
conformance change.

## Product and implementation matrix

| Resource | Declared canonical version | Active Edgion input path | Additional v1.6.2 served versions | Classification |
|----------|----------------------------|--------------------------|------------------------------------------------|----------------|
| GatewayClass | v1 | One center-lifetime v1-preferred transport shared by watch, CRUD, and status; v1beta1 converts before canonical processing | v1beta1 served and storage in the v1.6.2 bundle | v1 canonical; v1beta1 `ImplementedUnverified`, pending per-source bundle and conformance evidence |
| Gateway | v1 | One center-lifetime v1-preferred transport shared by watch, CRUD, and status; v1beta1 converts before canonical processing | v1beta1 served | v1 canonical; v1beta1 `ImplementedUnverified`, pending bundle and conformance evidence |
| HTTPRoute | v1 | One center-lifetime v1-preferred transport shared by watch, CRUD, and status; v1beta1 converts before canonical processing | v1beta1 served | v1 canonical; v1beta1 `ImplementedUnverified`, pending bundle and conformance evidence |
| GRPCRoute | v1 | Prefer v1; use v1alpha2 watch, CRUD, and status when only that version is served; skip when absent | none; v1alpha2 was last served in v1.1.1 | v1alpha2 `ImplementedUnverified` |
| TCPRoute | v1 | Prefer v1; use v1alpha2 watch, CRUD, and status when only that version is served; skip when absent | v1alpha2 served | v1alpha2 `ImplementedUnverified`; multiple rules are rejected and withdrawn |
| UDPRoute | v1 | Prefer v1; use v1alpha2 watch, CRUD, and status when only that version is served; skip when absent | v1alpha2 served | v1alpha2 `ImplementedUnverified`; multiple rules are rejected and withdrawn |
| TLSRoute | v1 | Prefer v1; use v1alpha3 watch, CRUD, and status when only that version is served; skip when absent | v1alpha2 and v1alpha3 served | v1alpha3 `ImplementedUnverified`; v1alpha2 unsupported |
| ReferenceGrant | v1 | One center-lifetime v1-preferred transport selection shared by watch and CRUD; v1beta1 converts before canonical processing | v1beta1 served and storage in the v1.6.2 bundle | v1 canonical; v1beta1 adapter wired for Kubernetes watch and offline/Admin input, pending per-source bundle and conformance evidence |
| BackendTLSPolicy | v1 | Prefer v1; use v1alpha3 watch, CRUD, and status when only that version is served; skip when absent | v1alpha3 served | v1alpha3 `ImplementedUnverified` |
| ListenerSet | unsupported | no processor or watcher | v1 served | CRD availability only; explicitly not advertised |

The Standard v1.6.2 bundle serves TCPRoute and UDPRoute v1. The Experimental
bundle adds other fields and resources within the same release. Edgion's installation script defaults to the Experimental v1.6.2 bundle.

## Ingress and evidence coverage

`gateway_api_support::SOURCES` records Kubernetes watch, dynamic CRUD, status
transport, raw TypeMeta guard, and converter separately. Its `SourceEvidence`
keeps tested Standard bundles, Experimental bundles, conformance suite, and
feature evidence independent. All those per-source evidence fields are empty
today; the inventory does not turn historical profile results into source GVK
claims. `Wired` records a current code binding, not successful
end-to-end compatibility with every bundle. All canonical rows have typed
watch/CRUD wiring. ReferenceGrant has no status subresource in this model.
Raw TypeMeta guards cover canonical Gateway API sources. GatewayClass, Gateway,
HTTPRoute, and ReferenceGrant v1beta1 pass explicit source checks and convert into their v1
models before the FileSystem, etcd, Admin, and Kubernetes processor boundaries.
Kubernetes Admin writes convert authored beta input and submit through the selected
transport; their status writes use that same transport where the resource has a
status subresource. Direct Kubernetes API
submission still requires the cluster CRD to serve the submitted version. Their
generated Rust roots keep the original wire kinds. Source adapters remain
`ImplementedUnverified` until their bundle, feature, and conformance evidence is complete.

The five last pre-v1 sources use an explicit raw TypeMeta guard and conversion
for FileSystem, etcd, and Admin. Admin validates against the selected v1.6.2
Experimental schema except GRPCRoute v1alpha2, whose last co-served v1.1.1
Experimental schema is embedded in the Controller binary. Kubernetes continues
to prefer v1 watch and CRUD and use the selected alpha version when v1 is not
served. If neither selected version is served, that kind is skipped without
failing Controller startup. TCPRoute and UDPRoute apply the v1 one-rule
limit after Kubernetes conversion, so an invalid alpha-authored update cannot
keep an earlier route published. These adapters have no per-source conformance
evidence yet.

No row carries per-source-version conformance evidence yet. Existing
Gateway API profile results are tied to a suite and feature set, not proof of
every source GVK, CRD bundle, or channel combination. Declared feature names
remain in `SUPPORTED_FEATURES` and `UNSUPPORTED_OR_SKIPPED_FEATURES` in
`edgion-resources/src/resources/gateway_class.rs`. The inventory exposes the
former through `declared_supported_features()` without copying it, and does
not infer per-source feature evidence from resource identity.

The offline OpenAPI fixture matrix executed all 13 exact stable releases from
v1.0.0 through v1.6.2 in both channels (26 locked official release artifacts):
266 minimal fixture passes, 72 source-GVK N/A rows, and zero failures. The
executed release and source rows are in
`edgion-tests/gateway_api_bundles/openapi-results.tsv`; URLs and SHA-256
digests are in `artifacts.json`. A shared Edgion Rust test decodes/preparses
the nine canonical source models and decodes/converts the four implemented
v1beta1 adapters. See that directory's README for the exact limits. These
are bounded fixture results, not complete field, semantic, CEL, or conformance
proof for any release. Kubernetes input also requires the installed CRD to
serve the submitted version.

Release-pair fixtures also exercise four concrete Standard v1 schema drifts:
HTTPRoute match-list bound v1.1.1→v1.2.0, Gateway address value newly optional
v1.2.1→v1.3.0, BackendTLSPolicy named well-known CA v1.4.1→v1.5.0, and
TLSRoute hostname-list bound v1.5.1→v1.6.0. The two newly decodable but
unsupported values (missing Gateway address value and non-System CA name)
produce per-object Controller rejection without blocking sibling LIST items;
they do not imply runtime support for allocating addresses or non-System
named CAs. Added field/default and validation-regex metadata are separately
recorded with exact paths in `schema-change-results.tsv`.

## ReferenceGrant v1beta1 schema window

The upstream Standard CRDs from [v1.0.0](https://github.com/kubernetes-sigs/gateway-api/blob/v1.0.0/config/crd/standard/gateway.networking.k8s.io_referencegrants.yaml) through [v1.6.2](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/config/crd/standard/gateway.networking.k8s.io_referencegrants.yaml) serve v1beta1 and
have no status subresource. Their required `spec.from` and `spec.to` fields,
from/to entry fields, defaults, and bounds remain the same. v1.4.0 adds
`x-kubernetes-list-type: atomic` to both lists; this changes Kubernetes merge
topology, not the decoded resource fields. v1 first appears in the checked
v1.5.0 CRD and has the same `spec` schema as v1beta1 in v1.5.0 and v1.6.2.
The v1.6.1 and v1.6.2 v1beta1 spec JSON SHA-256 hashes both equal
`3b199c8eddba3c8a8a54d40269c1ecb4620805a6d1c72de90aa10c07817dc6d5`;
their full CRD files differ in release metadata. These are schema audits, not per-version Edgion
integration or conformance results.

## HTTPRoute v1 contains mixed-stability fields

`gateway.networking.k8s.io/v1/HTTPRoute` is not one indivisible support unit.
The v1.6.2 Experimental bundle adds the following paths to the same v1 GVK when
compared with the v1.6.2 Standard bundle:

| Experimental-only path in the bundled HTTPRoute v1 schema | Current Edgion model | Current product claim |
|------------------------------------------------------------|----------------------|-----------------------|
| `spec.useDefaultGateways` | `All` retained for validation and blocked from serving; `None` follows ordinary attachment | Not advertised |
| `spec.rules[].retry` | Represented and consumed by retry preparation/runtime | Not advertised as a Gateway API conformance feature |
| `spec.rules[].sessionPersistence` | Represented and consumed by session-persistence runtime | Not advertised as a Gateway API conformance feature |
| `spec.rules[].filters[].externalAuth` | Upstream `backendRef`/`protocol` form retained and rejected per rule; Edgion's distinct ForwardAuth form remains an internal extension | Upstream feature unsupported |
| `spec.rules[].backendRefs[].filters[].externalAuth` | Same source-shape check and per-rule rejection | Upstream feature unsupported |

This is not an alternate `v1alpha*` HTTPRoute representation. These are
Experimental features carried inside `HTTPRoute/v1`, selected by the installed
bundle channel. Consequently, a support matrix must track GVK, bundle/channel,
and feature independently. Type presence alone is not a support declaration.

Other Extended features, such as CORS in the bundled v1.6.2 schemas, may be
present in both Standard and Experimental channels. Extended support and
Experimental channel membership are separate properties.

## Evidence locations

- Resource GVK declarations: `edgion-resources/src/resources/`.
- Canonical feature declarations:
  `edgion-resources/src/resources/gateway_class.rs`.
- Kubernetes version probing and preflight:
  `edgion-controller/src/conf_mgr/conf_center/kubernetes/version_detection.rs`.
- Watch selection:
  `edgion-controller/src/conf_mgr/conf_center/kubernetes/controller.rs`.
- Dynamic CRUD selection:
  `edgion-controller/src/conf_mgr/conf_center/kubernetes/storage.rs`.
- TLSRoute canonical type and ReferenceGrant conversion types:
  `edgion-resources/src/resources/{tls_route,reference_grant}.rs` and the
  corresponding Controller handlers.
- Bundled schemas: `config/crd/gateway-api/gateway-api-*-v1.6.2.yaml`.
- Historical GRPCRoute source schema: `config/gateway-api-source-schemas/grpcroute-v1alpha2-v1.1.1-experimental.yaml`.
- Conformance dependency: `edgion-tests/conformance/go.mod` (currently v1.6.2,
  pinned separately from the v1.6.2 CRD bundle).

## Known truthfulness gaps

- `GatewayClass` writes `SupportedVersion=Unknown` with reason
  `CompatibilityUnknown`. The Controller cannot verify installed CRD bundle
  and channel compatibility from its GVK-availability preflight.
- Kubernetes preflight checks whether selected GVKs are served, but does not
  inspect `gateway.networking.k8s.io/bundle-version` or channel annotations.
- Static/Admin TypeMeta is checked before typed parsing for registered Gateway
  API sources. Source GVK validation does not prove bundle-level field parity.
- The public documentation has historically conflated the v1.5.0 CRD bundle
  with the v1.5.1 conformance dependency. They are separate version claims.
- HTTPRoute retry and session persistence are represented in the implementation
  but are not listed in the canonical supported-feature set. Upstream
  ExternalAuth is rejected per rule. `useDefaultGateways: All` is rejected by
  Admin and withheld from serving; when a Kubernetes Route has no parentRef,
  there is no valid parent status entry on which to report Accepted=False.
- Further same-GVK schema drift beyond the focused fixture and field-path
  inventories has not been tested against the Edgion semantic/runtime path.
