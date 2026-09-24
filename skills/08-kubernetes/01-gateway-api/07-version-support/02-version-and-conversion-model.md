# Gateway API version and conversion model

## Version axes

Gateway API support has three independent axes:

1. A Kubernetes API version in the source GVK, such as HTTPRoute v1 or TLSRoute
   v1alpha3.
2. A Gateway API bundle version and channel, such as v1.5.0 experimental.
3. Field or feature stability inside that representation: Core, Extended, or
   Experimental.

The same GVK may have a slightly different schema in two bundle releases.
Standard-channel changes within an API version are designed to remain compatible,
but validation corrections, new fields, and optionality changes are still
possible. Experimental-channel minor releases may be breaking.

For example, the v1.5.0 Experimental bundle adds retry, session persistence,
ExternalAuth, and default-Gateway selection fields to `HTTPRoute/v1`. Calling
the GVK supported does not automatically claim all of those fields.

Authoritative upstream references:

- [Gateway API versioning](https://gateway-api.sigs.k8s.io/docs/concepts/versioning/)
- [Implementer's guide](https://gateway-api.sigs.k8s.io/guides/implementers-guide/)
- [CRD management](https://gateway-api.sigs.k8s.io/guides/crd-management/)

## Source-adapter contract

The active policy uses this definition:

> A source GVK is supported only when Edgion has a schema-backed decoder, an
> explicit conversion to the canonical resource model, full semantic/status
> handling after conversion, and tests for that path.

Conceptually:

```text
raw document or watched object
  -> verify source GVK
  -> validate against the source schema
  -> deserialize the source representation
  -> explicitly convert to the canonical model
  -> shared semantic validation and preparation
  -> status and publication through the source-version transport
```

The code-owned source inventory records full source GVKs, including proposed
and partial paths. Only an implemented, tested source path may be advertised.
The mere presence of a schema in a CRD bundle must not auto-register support.

The source adapter must also define a feature projection. A field accepted by
the selected source schema must either convert into supported canonical
behavior or receive the Gateway API status required for unsupported behavior;
it must not be silently dropped merely because the surrounding GVK is known.

## Kubernetes-owned and Edgion-owned conversion

In Kubernetes mode, the API server may accept one served version and convert it
to the version watched by Edgion. That can provide a valid source path, but the
claim remains conditional on the installed CRD's served/storage/conversion
contract and needs cluster tests.

In filesystem, etcd, Admin API, and offline validation there is no API server.
Any alternate source version therefore needs an Edgion-owned decoder and
conversion. Deserializing an older document directly into the canonical Rust
type after ignoring `apiVersion` is not conversion and must not be described as
support.

## Same-GVK evolution

An HTTPRoute v1 document does not identify the bundle release whose schema the
author used. A non-Kubernetes deployment therefore needs one deterministic
validation contract shipped with the Edgion release. Forward compatibility for
standard fields should tolerate unknown additions and optional fields, while
known fields retain semantic validation required by the canonical runtime.

Bundle metadata remains relevant for Kubernetes preflight and conformance, even
when the source GVK stays v1. It must not be used as a shortcut for proven
runtime support.

For non-Kubernetes input, selecting the Standard schema is a product policy,
not a way to validate Experimental fields already represented by Edgion. If
Edgion elects to support such a field, the shipped schema profile and feature
declaration must make that choice explicit.

## Status and failure requirements

- Unsupported source GVK: reject before typed deserialization or publication.
- Schema-invalid but identifiable static resource: do not publish; expose a
  stable error status where the backend supports status sidecars.
- Semantically invalid canonical resource: use the owning Gateway API status
  conditions and retain valid sibling rules where the specification permits.
- Incompatible Kubernetes bundle: `SupportedVersion` must not remain true.
- Gateway defensive construction must fail closed if invalid state bypasses the
  Controller; it must not implement a second version-conversion system.

## Current implementation boundary

The inventory is a policy and consistency aid, not a source decoder.
ReferenceGrant v1beta1 is converted to the canonical v1 resource before raw
store insertion for Kubernetes watch and before typed processing for offline
and Admin input. The center selects one Kubernetes transport version for its
lifetime; a cluster must serve v1beta1 for that Kubernetes input to be accepted.
Per-source bundle and conformance evidence remains incomplete. The selected
GRPCRoute v1alpha2, TLSRoute v1alpha3, BackendTLSPolicy v1alpha3, TCPRoute
v1alpha2, and UDPRoute v1alpha2 raw sources convert to canonical v1 for
non-Kubernetes input. Kubernetes watch, CRUD, and status use one selected
transport per kind: v1 when served, otherwise the registered alpha version.
If neither is served, the kind is skipped without failing Controller startup.
