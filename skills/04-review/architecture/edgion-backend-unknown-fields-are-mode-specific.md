# EdgionBackend unknown-field handling is mode-specific

## Decision

Documentation must not describe `deny_unknown_fields` as a uniform admission
guarantee. `EdgionBackend` follows the project-wide schema/transport contract:

- Kubernetes uses a structural CRD. Requests using
  `fieldValidation=Strict` reject undeclared `spec` fields, while Warn/Ignore
  clients may allow the API server to prune them before the Controller sees
  the object. A pruned typo can therefore be a silent no-op, but the pinned
  Kubernetes acceptance gate uses strict server-side validation.
- Filesystem and etcd input reaches serde directly, where the strict spec and
  nested structures reject fields covered by `deny_unknown_fields`.

This is a documentation correction, not a request to preserve unknown fields or
duplicate Kubernetes admission inside the Controller. The canonical structural
schema rule remains
[Resource schema and transport contract](../../01-architecture/05-resources/00-schema-transport-contract.md).

## Coverage

The canonical feature reference and the English and Chinese operator pages all
state the same mode-specific behavior. The committed CRD remains structural,
the Rust spec remains strict for direct deserialization, and the CRD does not
use the Kubernetes-1.26-invalid combination of `properties` with
`additionalProperties: false`.
