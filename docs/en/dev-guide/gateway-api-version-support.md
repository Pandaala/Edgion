# Gateway API Version Support

Edgion treats a resource API version and a Gateway API bundle version as two different concepts. This page describes the version boundaries you can rely on today and the compatibility that must not be inferred from them.

## Currently declared resource versions

| Resource | Edgion-declared version | Notes |
|----------|-------------------------|-------|
| GatewayClass | `gateway.networking.k8s.io/v1` | Core |
| Gateway | `gateway.networking.k8s.io/v1` | Core |
| HTTPRoute | `gateway.networking.k8s.io/v1` | Core |
| GRPCRoute | `gateway.networking.k8s.io/v1` | Core |
| TLSRoute | `gateway.networking.k8s.io/v1` | Standard; v1 only |
| ReferenceGrant | `gateway.networking.k8s.io/v1` | Standard; the Kubernetes path can still probe v1beta1 |
| BackendTLSPolicy | `gateway.networking.k8s.io/v1` | v1 only |
| TCPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |
| UDPRoute | `gateway.networking.k8s.io/v1alpha2` | Experimental |

A version being exposed by the cluster CRD does not mean that Edgion declares support for it. Complete support also requires the corresponding read path, type conversion, semantic handling, status update, and tests.

## Bundle and API versions

The repository currently carries the Gateway API v1.5.0 CRD bundle, while the conformance-test dependency is v1.5.1. These version numbers describe different artifacts and must not be combined into one support claim.

The same GVK, such as `gateway.networking.k8s.io/v1/HTTPRoute`, may have a slightly different schema in different bundles. The Standard channel requires compatible evolution; the Experimental channel does not provide the same guarantee.

## Feature stability inside HTTPRoute v1

The `v1` label applies to the HTTPRoute API representation, not to every optional feature inside it. In the bundled Gateway API v1.5.0 CRDs, the Experimental channel adds `spec.useDefaultGateways`, `spec.rules[].retry`, `spec.rules[].sessionPersistence`, and `externalAuth` rule/backend filters to the same `HTTPRoute/v1` GVK.

Edgion currently has model and runtime support for retry, session persistence, and ExternalAuth, but does not model `useDefaultGateways`. Those implemented fields are not currently advertised as Gateway API conformance features. Therefore, resource-version support, schema acceptance, runtime implementation, and `GatewayClass.status.supportedFeatures` are separate claims.

Do not describe these fields as a separate HTTPRoute alpha API version. They are Experimental features inside the v1 representation. Extended features are also not necessarily Experimental-channel-only; for example, the bundled CORS field is present in both channels.

## Kubernetes mode

The Kubernetes API server applies the schema, defaulting, field pruning, and CEL validation from the CRD installed in the cluster. The Edgion Controller then reads objects through its selected API version and applies implementation-level semantic validation.

The current startup check confirms only that required GVKs are served. It does not yet establish full compatibility from the CRD's `gateway.networking.k8s.io/bundle-version` and channel annotations. Do not treat an arbitrary Gateway API CRD update as validated by Edgion.

## Non-Kubernetes mode

TLSRoute documents must explicitly declare `apiVersion: gateway.networking.k8s.io/v1`.
Filesystem, etcd, and Admin inputs reject missing, malformed, and older TLSRoute
API versions. Change older manifests to v1 before loading them; there is no
automatic legacy conversion.

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
