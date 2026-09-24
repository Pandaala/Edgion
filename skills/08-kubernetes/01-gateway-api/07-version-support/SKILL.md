---
name: gateway-api-version-support
description: Determine whether a Gateway API source version is actually supported by Edgion, and trace schema validation, conversion to the canonical model, Kubernetes CRD compatibility, and conformance evidence.
---

# Gateway API version support

Use this section when changing Gateway API versions, schemas, watchers,
non-Kubernetes ingestion, conversion adapters, `SupportedVersion`, or
conformance claims.

## Read by question

| Question | Read |
|----------|------|
| What is declared, observed, or merely served by the bundled CRDs? | [Current support matrix](01-current-support-matrix.md) |
| What must exist before a source GVK can be called supported? | [Version and conversion model](02-version-and-conversion-model.md) |
| Where does validation run today, and what does it miss? | [Validation current state](03-validation-current-state.md) |

## Required distinctions

Never use one of these as a synonym for another:

- **Source GVK**: the group, version, and kind submitted or watched.
- **Canonical model**: the single typed representation consumed after ingress.
- **CRD served version**: a version the installed API server accepts; this does
  not prove Edgion can consume it outside API-server conversion.
- **Bundle version and channel**: the particular Gateway API CRD release and
  its standard or experimental compatibility contract.
- **Field and feature stability**: Core, Extended, or Experimental behavior
  inside a resource representation. A `v1` GVK can still contain fields shipped
  only in the Experimental channel.
- **Declared support**: an intentional product statement backed by a complete
  ingest/conversion/status path and tests.
- **Conformance evidence**: results tied to one Gateway API conformance-suite
  version and the features actually executed.

Serde accepting a document after discarding `apiVersion`, or Kubernetes serving
a version, is not sufficient evidence of Edgion support.

## Current policy and staged implementation

The active [API version policy](../../04-api-version-upgrade.md) keeps one
canonical model per kind and allows individually tested source adapters.
The code-owned inventory is `edgion-resources/src/gateway_api_support.rs`.
Its `Proposed` and `PartialKubernetes` entries are not support claims. The
staged work in `tasks/todo/gateway-api-version-compatibility-and-validation/`
must complete each adapter before changing that classification.

## Review checklist

Before changing a Gateway API version boundary:

1. Identify the source GVK, bundle/channel, and field/feature stability
   independently.
2. Locate the exact schema used at each ingest path.
3. Verify TypeMeta before typed deserialization can discard it.
4. Trace API-server or Edgion-owned conversion into the canonical model.
5. Trace watch, CRUD, status patch, static input, and conf-sync identities.
6. Verify semantic validation and preparation after conversion.
7. Check `GatewayClass.status.conditions[SupportedVersion]`,
   `status.supportedFeatures`, and field-level claims.
8. Add source-version, conversion, rejection, recovery, and conformance tests
   appropriate to the claimed support.
