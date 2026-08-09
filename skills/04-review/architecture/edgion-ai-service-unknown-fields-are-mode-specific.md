# EdgionAIService unknown-field handling is mode-specific

## Decision

Documentation must not describe `deny_unknown_fields` as a uniform admission
guarantee. `EdgionAIService` follows the project-wide schema/transport contract:

- Kubernetes uses a structural CRD. The API server prunes undeclared optional
  `spec` fields before the Controller sees them, so such a typo may be a silent
  no-op with no status condition. Pruning that leaves a required field absent,
  or another schema constraint unsatisfied, still fails admission.
- Filesystem and etcd input reaches serde directly, where the strict spec and
  nested structures reject fields covered by `deny_unknown_fields`.

This is a documentation correction, not a request to preserve unknown fields or
duplicate Kubernetes admission inside the Controller. The canonical structural
schema rule remains
[Resource schema and transport contract](../../01-architecture/05-resources/00-schema-transport-contract.md).

## Coverage

The canonical feature reference and the English and Chinese operator pages all
state the same mode-specific behavior. The committed CRD remains structural and
the Rust spec remains strict for direct deserialization.
