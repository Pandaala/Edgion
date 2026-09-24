# Gateway API Version Support

Edgion treats a resource API version and a Gateway API bundle version as two different concepts. This page describes the version boundaries you can rely on today and the compatibility that must not be inferred from them.

## Currently declared resource versions

| Resource | Edgion-declared version | Notes |
|----------|-------------------------|-------|
| GatewayClass | `gateway.networking.k8s.io/v1` | Core; a v1beta1 source adapter exists but lacks per-source conformance evidence |
| Gateway | `gateway.networking.k8s.io/v1` | Core; a v1beta1 source adapter exists but lacks per-source conformance evidence |
| HTTPRoute | `gateway.networking.k8s.io/v1` | Core; a v1beta1 source adapter exists but lacks per-source conformance evidence |
| GRPCRoute | `gateway.networking.k8s.io/v1` | Core |
| TLSRoute | `gateway.networking.k8s.io/v1` | Standard; v1 only |
| ReferenceGrant | `gateway.networking.k8s.io/v1` | Standard; a v1beta1 source adapter exists but lacks per-source conformance evidence |
| BackendTLSPolicy | `gateway.networking.k8s.io/v1` | v1 only |
| TCPRoute | `gateway.networking.k8s.io/v1` | Standard |
| UDPRoute | `gateway.networking.k8s.io/v1` | Standard |

## v1.6.2 conformance evidence

In an isolated kind cluster with the v1.6.2 Experimental CRDs and Go
conformance v1.6.2, the full `GATEWAY-HTTP` profile passed Core 37/37 and
Extended 30/30; `GATEWAY-GRPC` passed Core 15/15. The full `GATEWAY-TLS`
profile passed Core 20/20 and Extended 2/3. TLS termination passed its
targeted case but timed out in the full profile, so Edgion does not advertise
`TLSRouteModeTerminate` as a complete conformance feature. The
[`HTTP/GRPC report`](../../../edgion-tests/conformance/reports/gateway-http-grpc-v1.6.2-kind-2026-09-23.yaml)
and [`TLS report`](../../../edgion-tests/conformance/reports/gateway-tls-v1.6.2-kind-2026-09-23.yaml)
record the exact results.

TCPRoute/v1 and UDPRoute/v1 accepted routes and forwarded real traffic on
ports bound at Gateway startup. Their full v1.6.2 profiles timed out when
tests created new listener ports after startup. The Gateway must restart to
bind those ports, so `TCPRoute` and `UDPRoute` are withheld from
`GatewayClass.status.supportedFeatures` pending full profile passes. The
remaining [TLS](../../../tasks/pending/gateway-api-v16-tls-terminate-full-profile.md)
and [dynamic-listener](../../../tasks/pending/gateway-api-v16-dynamic-stream-listeners.md)
work is tracked separately. These results do not establish complete v1.6.2
conformance or compatibility with arbitrary installed CRD bundles.

A version being exposed by the cluster CRD does not mean that Edgion declares support for it. Complete support also requires the corresponding read path, type conversion, semantic handling, status update, and tests.

## Bundle and API versions

The repository currently carries Gateway API v1.6.2 CRD bundles. The Go
conformance module depends on `sigs.k8s.io/gateway-api` v1.6.2 and
`sigs.k8s.io/gateway-api/conformance` v1.6.2. These are separate artifacts.

An offline fixture matrix checked exact official Standard and Experimental
release artifacts for all 13 stable tags from v1.0.0 through v1.6.2. Across
the inventoried source GVKs, 266 minimal OpenAPI fixture rows passed and 72
were N/A because that source GVK was not served. The fixture corpus also
decodes the nine canonical Edgion source models and converts the four wired
v1beta1 source adapters in Rust. The fixture evidence in
`edgion-tests/gateway_api_bundles/README.md`
records exact releases, URLs, SHA-256 digests, and field-path presence.
Minimal fixture acceptance does not establish compatibility for every field
or installed CRD bundle; CEL and full Controller/Gateway behavior were not
part of this offline matrix.

The matrix also checks exact Standard release transitions. Gateway/v1 permits
an address without `value` from v1.3.0; Edgion decodes it, reports
`Accepted=False`, and withholds that Gateway's listeners because it cannot
assign an address. BackendTLSPolicy/v1 permits CA names beyond `System` from
v1.5.0; Edgion decodes them, reports an unsupported value, and withholds the
policy. Sibling resources in the same List remain processable. The fixture
README records the other tested bounds, added fields, and defaults.

Gateway v1.6.2 permits up to 16 `spec.tls.frontend` CA references. Edgion's
decoder, schema, Controller resolution, and Gateway TLS compiler use the same
16-reference limit; the separate aggregate CA-material limits still apply.

The same GVK, such as `gateway.networking.k8s.io/v1/HTTPRoute`, may have a slightly different schema in different bundles. The Standard channel requires compatible evolution; the Experimental channel does not provide the same guarantee.

## Feature stability inside HTTPRoute v1

The `v1` label applies to the HTTPRoute API representation, not to every optional feature inside it. In the bundled Gateway API v1.6.2 CRDs, the Experimental channel adds `spec.useDefaultGateways`, `spec.rules[].retry`, `spec.rules[].sessionPersistence`, and `externalAuth` rule/backend filters to the same `HTTPRoute/v1` GVK.

Edgion has model and runtime support for retry and session persistence, but
does not currently advertise them as Gateway API conformance features. It
retains `useDefaultGateways` so `All` can be rejected instead of silently
ignored. It also retains the upstream ExternalAuth `backendRef`/`protocol`
shape so the affected rule can be rejected; Edgion's separate ForwardAuth
extension has a different config shape. Therefore, resource-version support,
schema acceptance, runtime implementation, and
`GatewayClass.status.supportedFeatures` are separate claims.

Do not describe these fields as a separate HTTPRoute alpha API version. They are Experimental features inside the v1 representation. Extended features are also not necessarily Experimental-channel-only; for example, the bundled CORS field is present in both channels.

## Kubernetes mode

The Kubernetes API server applies the schema, defaulting, field pruning, and CEL validation from the CRD installed in the cluster. The Edgion Controller then reads objects through its selected API version and applies implementation-level semantic validation.

The current startup check confirms only that required GVKs are served. It does
not establish compatibility from CRD bundle/version and channel annotations.
Consequently, `GatewayClass.status.conditions[SupportedVersion]` is `Unknown`
with reason `CompatibilityUnknown`; it does not assert that the installed
bundle is verified. Do not treat an arbitrary Gateway API CRD update as
validated by Edgion.

The [Gateway API implementer's guide](https://gateway-api.sigs.k8s.io/guides/implementers-guide/#version-compatibility)
defines `True` as a supported installed CRD version and `False` as an
unsupported one. `Unknown` records that Edgion has not established either
conclusion for the installed bundle; it is not a conformance result.

## Non-Kubernetes mode

Gateway API documents must declare their exact supported source `apiVersion`
and `kind`. Filesystem, etcd, and Admin inputs reject missing, malformed, or
unregistered Gateway API TypeMeta. TLSRoute is v1 only; change older manifests
to v1 before loading them. Implemented v1beta1 adapters for GatewayClass,
Gateway, HTTPRoute, and ReferenceGrant convert into canonical v1 models.
Gateway conf-sync accepts only canonical source TypeMeta from the Controller.

Filesystem, etcd, and Admin API ingestion do not have a Kubernetes API server to perform version conversion. To accept a source version other than the canonical version in the future, Edgion must provide its schema, source type, and explicit conversion. Successful deserialization after ignoring `apiVersion` is not version support.

## Compatibility contract

- Use the versions declared in the table for current configuration.
- Do not rely on additional served CRD versions unless Edgion explicitly documents and tests them.
- A Standard-channel field unknown to an older Edgion build may be ignored; an ignored field provides no runtime behavior.
- Experimental resources and fields may change incompatibly across Gateway API bundle upgrades.
- Check field-level feature support even when the surrounding resource GVK is supported.
- Upgrade the Gateway API CRDs and Edgion as one coordinated change.

## Related documentation

- [Gateway API versioning](https://gateway-api.sigs.k8s.io/docs/concepts/versioning/)
- [Gateway API CRD management](https://gateway-api.sigs.k8s.io/guides/crd-management/)
- [Resource architecture overview](./resource-architecture-overview.md)
