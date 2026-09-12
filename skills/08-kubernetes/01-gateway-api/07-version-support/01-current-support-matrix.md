# Current Gateway API version support matrix

This is a static audit of the repository on 2026-09-12. It separates declared
product support from compatibility code and versions merely exposed by the
vendored Gateway API v1.5.0 CRDs. Revalidate it after any type, watcher, CRD, or
conformance change.

## Product and implementation matrix

| Resource | Declared canonical version | Active Edgion input path | Additional v1.5.0 experimental CRD versions | Classification |
|----------|----------------------------|--------------------------|------------------------------------------------|----------------|
| GatewayClass | v1 | v1 typed watch and CRUD | v1beta1 served | v1 declared; v1beta1 relies on API-server conversion and is not an Edgion adapter |
| Gateway | v1 | v1 typed watch and CRUD | v1beta1 served | v1 declared; v1beta1 relies on API-server conversion and is not an Edgion adapter |
| HTTPRoute | v1 | v1 typed watch and CRUD | v1beta1 served | v1 declared; no explicit Edgion v1beta1 decoder/converter |
| GRPCRoute | v1 | v1 typed watch and CRUD | none | v1 declared |
| TCPRoute | v1alpha2 | v1alpha2 typed watch and CRUD | none | Experimental source and canonical model are currently the same type |
| UDPRoute | v1alpha2 | v1alpha2 typed watch and CRUD | none | Experimental source and canonical model are currently the same type |
| TLSRoute | v1 | v1 preferred; v1alpha3 fallback watcher, static conversion, and compatibility handler | v1alpha2 and v1alpha3 served | v1 declared; v1alpha3 compatibility exists but conflicts with the current atomic-version policy; v1alpha2 has no direct Edgion adapter |
| ReferenceGrant | v1 | v1 preferred; v1beta1 fallback watcher and converter | v1beta1 served and storage in the v1.5.0 bundle | v1 declared; v1beta1 compatibility exists and converts into the v1 store model |
| BackendTLSPolicy | v1 | v1-only watch, CRUD, and raw TypeMeta guard | v1alpha3 served | v1 declared; v1alpha3 intentionally has no Edgion adapter |
| ListenerSet | unsupported | no processor or watcher | v1 served | CRD availability only; explicitly not advertised |

The standard v1.5.0 bundle differs from the experimental bundle. In particular,
the standard bundle does not serve TCPRoute or UDPRoute, and marks several alpha
versions unserved. Edgion's installation script currently defaults to the
experimental v1.5.0 bundle.

## HTTPRoute v1 contains mixed-stability fields

`gateway.networking.k8s.io/v1/HTTPRoute` is not one indivisible support unit.
The v1.5.0 Experimental bundle adds the following paths to the same v1 GVK when
compared with the v1.5.0 Standard bundle:

| Experimental-only path in the bundled HTTPRoute v1 schema | Current Edgion model | Current product claim |
|------------------------------------------------------------|----------------------|-----------------------|
| `spec.useDefaultGateways` | Not represented | Not advertised |
| `spec.rules[].retry` | Represented and consumed by retry preparation/runtime | Not advertised as a Gateway API conformance feature |
| `spec.rules[].sessionPersistence` | Represented and consumed by session-persistence runtime | Not advertised as a Gateway API conformance feature |
| `spec.rules[].filters[].externalAuth` | Represented and mapped to the ForwardAuth runtime | Not advertised as a Gateway API conformance feature |
| `spec.rules[].backendRefs[].filters[].externalAuth` | Represented through the shared filter shape and runtime | Not advertised as a Gateway API conformance feature |

This is not an alternate `v1alpha*` HTTPRoute representation. These are
Experimental features carried inside `HTTPRoute/v1`, selected by the installed
bundle channel. Consequently, a support matrix must track GVK, bundle/channel,
and feature independently. Type presence alone is not a support declaration.

Other Extended features, such as CORS in the bundled v1.5.0 schemas, may be
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
- TLSRoute and ReferenceGrant conversion types and handlers:
  `edgion-resources/src/resources/{tls_route,reference_grant}.rs` and the
  corresponding Controller handlers.
- Bundled schemas: `config/crd/gateway-api/gateway-api-*-v1.5.0.yaml`.
- Conformance dependency: `edgion-tests/conformance/go.mod` (currently v1.5.1,
  distinct from the v1.5.0 CRD bundle).

## Known truthfulness gaps

- `GatewayClass` currently writes `SupportedVersion=True` unconditionally; it
  does not compare the installed CRD bundle annotation or schema with a tested
  support range.
- Kubernetes preflight checks whether selected GVKs are served, but does not
  inspect `gateway.networking.k8s.io/bundle-version` or channel annotations.
- Static/Admin TypeMeta is explicitly enforced only for BackendTLSPolicy;
  TLSRoute has a special v1alpha3 conversion. Other typed parses can discard a
  missing or mismatched `apiVersion` before it is evaluated.
- The public documentation has historically conflated the v1.5.0 CRD bundle
  with the v1.5.1 conformance dependency. They are separate version claims.
- HTTPRoute retry, session persistence, and ExternalAuth are represented in the
  implementation but are not listed in the canonical supported-feature set;
  `useDefaultGateways` is neither represented nor advertised.
