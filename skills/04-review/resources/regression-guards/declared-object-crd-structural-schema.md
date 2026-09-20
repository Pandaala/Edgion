---
name: declared-object-crd-structural-schema
description: Use when reviewing Kubernetes CRD object schemas that declare named properties, especially EdgionTls clientAuth.allowedSans entries.
knowledge_type: regression-guards
---

# Declared CRD objects use Kubernetes structural schema

## Conclusion and scope

An object node in a committed structural CRD must not combine named `properties`
with `additionalProperties`. Kubernetes rejects that combination before any custom
resource can be admitted. The four fully structural Edgion CRDs are covered by the
field-presence contract, but field presence alone does not establish that their
OpenAPI schema is structurally valid.

The correction that established this guard applies to
`EdgionTls.spec.clientAuth.allowedSans.items`. Its named `type`, `match`, `value`,
`ignoreCase`, and `oid` fields, required fields, enums, defaults, and scalar
constraints remain unchanged.

## Rationale and evidence

The canonical authority is the
[resource schema and transport contract](../../../01-architecture/05-resources/00-schema-transport-contract.md).
Kubernetes v1.33.9 rejected the former schema because `allowedSans.items` declared
both `properties` and `additionalProperties: false`. Removing only the latter makes
the CRD installable while retaining the typed operator contract. Kubernetes handles
undeclared fields according to its structural-schema field-validation and pruning
rules; direct filesystem and etcd deserialization remains strict because
`AllowedSan` retains `#[serde(deny_unknown_fields)]`.

`edgion-resources/tests/crd_coverage.rs::tls_crd_is_recursively_structural` walks
the committed EdgionTls spec and rejects any declared object that also uses
`additionalProperties` or `x-kubernetes-preserve-unknown-fields`.

## Alternatives and limits

Do not restore `additionalProperties: false` to emulate serde strictness: it makes
the CRD itself invalid. Do not replace the typed node with
`x-kubernetes-preserve-unknown-fields`; that would weaken admission and permit
misspelled fields to reach later validation stages.

The recursive guard checks the incompatible object-keyword combinations that caused
this incident. The existing lockstep coverage test separately compares Rust and CRD
property names. Neither static check replaces an API-server validation when a live
Kubernetes environment is available.

## Verification and re-evaluation

The focused recursive guard passed after first demonstrating that it failed on the
original schema. A Kubernetes v1.33.9 server-side dry-run accepted the corrected
EdgionTls CRD. Re-evaluate this rule if Kubernetes changes its CRD structural-schema
contract or if the committed CRDs move to generated schemas.

## Resolution references

- `edgiontls-allowed-sans-nonstructural-crd` — fixed on 2026-09-20 by correcting the committed schema and adding recursive coverage.
- `edgion-tls-allowed-sans-structural-crd` — duplicate task consolidated into the same correction.
