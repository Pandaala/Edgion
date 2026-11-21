---
name: wasm-secret-refs-status-derivation
description: Use when reviewing Wasm fetch/OCI owning reference resolution, activity, authorization and usable-material status.
status: fixed
finding_id: contract-02
closed: 2026-07-18
knowledge_type: regression-guards
---

# Wasm fetch and OCI reference status

## Current boundary

`handlers/plugins/resolve_credential/wasm.rs::{resolve,status_errors}` consume
actual owning fields through `PreparedPluginReferences`. Both use the same named
auth/pull/TLS roles, canonical identity checks and bound Controller pipeline.
Unsupported group/kind or malformed identity cannot acquire same-name material
or register a dependency. Custom core defaults and the approved `core` alias
apply only through the owning field contract, before exact ReferenceGrant checks.

All four stage lists clear previous or externally supplied resolved material.
Only enabled entries in supported Wasm stages acquire material or report reference
errors. The final upstream-response stage does not support Wasm. Disabled fetch
TLS suppresses CA/client acquisition independently of fetch auth and OCI pull.

## Status and material parity

Read-only status evaluates identity, authorization, object existence and the same
`read_secret_utf8` usability predicate as parse. It reports invalid identity even
without a parse dependency edge, and reports missing objects itself. The handler's
exact final deduplication prevents duplicate messages from current owning fields. This
supersedes the historical rule forbidding missing-object reports in the Wasm
classifier, which depended on the old raw dependency scan. R4b removes that scan:
registry edges remain recovery indexes and cannot authorize readonly status
lookups after fields are changed, disabled or invalidated. CredentialInjector
also reports its own missing/denied outcomes rather than relying on the scan.

Fetch auth missing, empty or invalid UTF-8 data keys produce DecodeError. OCI
requires type `kubernetes.io/dockerconfigjson` and a nonempty UTF-8
`.dockerconfigjson` value. Wrong type produces InvalidSecretType; unusable data
produces DecodeError. TLS status classifies identity, authorization and presence;
it does not claim PEM parsing parity with Gateway client preparation.

CA ConfigMap and Secret dependencies keep separate keys (`ConfigMap/ns/name`
versus `ns/name`). Each target kind retains its exact Grant query, so a Secret-only
grant cannot authorize a same-name ConfigMap. Valid denied refs register recovery
edges before authorization, and handler clear-before-add rebuilds those edges.

## Accepted content limit

OCI validation remains shallow. Registry-host matching, JSON credentials and auth
selection are Gateway pull responsibilities and fail closed there. The existing
accepted status-accuracy limitation does not establish that every type-correct
operator document is valid. Re-evaluate it when concrete repeated failure cases
or a suitable shared parser justify extraction without a Controller-to-Gateway
crate dependency.

## Evidence and re-evaluation

The original July 18 false healthy signal came from checking existence alone.
July 22 added TLS Grant classification; September 15 preserved ConfigMap kind
in dependency keys and event requeue. October owning-field migration replaces
`resolve_wasm_refs` and its free raw classifiers with `wasm::status_errors`.

Existing `edgion_plugins` Wasm tests exercise usable/unusable auth, OCI type/data,
exact Grant denial, ConfigMap grant/status and ConfigMap event requeue. New handler
activity coverage checks supported/unsupported stages, disabled entries, and auth
or pull independence from disabled TLS. Overall reference manifest closure and
Common owning material/replay proof were completed under the shared
[resource reference contract](../../../01-architecture/00-common/06-resource-reference-contract.md). Fresh test results for
this slice were recorded in the closed resource-reference task (git history at
`d4fb7f4ad`).
R4b focused Controller verification passed 935 tests, including canonical-call
positive/zero controls, stale-edge isolation and actual Secret/ConfigMap worker
recovery through same-UID updates and delete/recreate. Observer counts concern
canonical authorization/store calls, not internal Grant-map reads.
Re-evaluate when supported stages, field roles, activity rules or transport
material semantics change. Historical verdicts do not suppress a current defect.

## Historical references

- `tasks/review-wasm-issues/contract-02-invalid-wasm-secrets-report-resolved.md`
  (closed 2026-07-18).
- [Wasm cache runtime identity](../../wasm/regression-guards/wasm-module-cache-key-runtime-config.md).
- [Historical OCI purge clamp](../../cpu-memory/superseded/wasm-oci-purge-interval-zero-clamp.md).
