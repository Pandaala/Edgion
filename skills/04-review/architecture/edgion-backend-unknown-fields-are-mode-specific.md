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

## Not the whole story for preserve-unknown-fields resources

This entry describes `EdgionBackend`'s admission-time behavior only. It is no longer a
complete account of "what happens to an unrecognized key" project-wide: `EdgionPlugins`,
`EdgionStreamPlugins`, typed `EdgionConfigData`, and `LinkSys` provider configurations carry
active ingest-side unknown-key DETECTION
(`edgion-resources::schema_check`, see
`serde-ignored-blind-spots-schema-walk-instead.md`), which exists precisely because their typed
payloads sit below `x-kubernetes-preserve-unknown-fields` boundaries — the one case this entry's
mode-specific pruning/rejection story does not cover at all, since the API server never
prunes that subtree in any mode. That detector warns (a status condition plus a log line)
rather than rejecting, and is a separate mechanism from both structural-CRD pruning and
`deny_unknown_fields`. See `skills/04-review/architecture/serde-ignored-blind-spots-schema-walk-instead.md`
and `skills/04-review/deserialization/SKILL.md` for the detector; this file's own
mode-specific pruning/rejection description still holds for `EdgionBackend` and any other
fully-structural CRD.

## Coverage

The canonical feature reference and the English and Chinese operator pages all
state the same mode-specific behavior. The committed CRD remains structural,
the Rust spec remains strict for direct deserialization, and the CRD does not
use the Kubernetes-1.26-invalid combination of `properties` with
`additionalProperties: false`.
