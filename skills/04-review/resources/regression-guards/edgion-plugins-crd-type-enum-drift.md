---
name: edgion-plugins-crd-type-enum-drift
description: Open HTTP and Stream plugin discriminators, preserved closed condition KeyGet enums, and the guards preventing schema drift.
status: fixed
finding_id: resource-02, key-get-trace-id-condition-crd-drift
closed: 2026-06-16
knowledge_type: regression-guards
---

# Plugin type admission and nested enum regression guards

## Current contract

All four HTTP and both Stream stage-entry `type` fields are open strings. A new plugin name
requires no CRD update; the binary owns supported types and stage validation. Unsupported
names must produce explicit Controller invalid status and fail-closed Gateway behavior, as
specified in [[unknown-plugin-variant-strict-enum]]. Do not restore a whitelist or substitute
unknown-field warnings for this failure path.

`edgion-resources/tests/plugin_crd_type_coverage.rs` and
`edgion-resources/tests/stream_plugin_crd_type_coverage.rs` guard open string discriminators
and config preservation. The HTTP guard retains schema-driven stage-exception checks and
checks that all three families exclude `Unsupported` from their supported-name catalogs.

Nested condition `KeyGet.type` remains a separate closed-enum boundary. Its values are still
compared with `schema_for!(KeyGet)` by `plugin_crd_condition_coverage.rs`. Removing plugin
name lists does not authorize removing condition enums, credential checks, or config structure.

## Historical evidence

The original `resource-02` correction reconciled missing `UrlRewrite`/`FaultInjection`, stale
`ExtensionRef`/`WebhookKeyGet`, and duplicate `ForwardAuth` whitelist entries. Per-stage
whitelist parity assertions became obsolete when the user authorized open plugin names.
Wrong-stage known plugins continue to be rejected by typed validation and runtime construction.

The separate `key-get-trace-id-condition-crd-drift` correction added `traceId` to the shared
condition enum and updated snapshots. Its bidirectional condition guard remains active. New
KeyGet variants still require their own schema and snapshot changes.

## Verification boundary

Historical September 11 checks are not evidence for this change. See the current implementation
record under `tasks/todo/http-plugin-wiring-plan/03-implementation-plan.md` for fresh results.
Re-evaluate when stage structure, the supported-type schema extraction, or nested condition
accessors change. No live Kubernetes test is implied by a passing static schema guard.
