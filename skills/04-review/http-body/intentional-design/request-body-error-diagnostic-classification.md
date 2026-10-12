---
name: request-body-error-diagnostic-classification
description: Use when reviewing shared body-error diagnostics versus operation-specific HTTP failure policy.
knowledge_type: intentional-design
---

# Request-body error diagnostics have one runtime classifier

## Accepted ownership and scope

`plugins/runtime/body_gate.rs::record_body_read_error_status` owns the shared
`BodyReadError` to `EdgionErrorCode` classification. The body gate,
JsonSchemaValidation and FormJsonTransform reuse it. Despite the historical
access-status name, the helper records a bounded error diagnostic; it does not
select the final HTTP status. `request_body/contracts.rs` still owns the error
type, and the adapter owns admission and precise refusal reasons.

ARCH-029 removed the identical private FormJsonTransform classifier and reused
the existing helper at its length, read, overwrite and rollback failure sites.
This removes a second semantic maintenance owner without adding an API,
allocation, lock or body copy. Retaining the duplicate with parity tests was
possible, but would keep the obligation to synchronize diagnostic changes.

## Preserved boundaries

Mapped failures record once at each existing site. `NotBuffered` adds no
diagnostic because the adapter records the precise refusal reason. Consumers
retain operation-specific HTTP replies and static PluginLog messages. A failed
FormJsonTransform length check still selects 500; a physical-cap read or
overwrite failure selects 413; a failed rollback retains the selected 500 even
when its diagnostic is `BodyReadTooLarge`.

The change preserves early media/length checks, shared complete request-body
storage, physical/logical caps and atomic overwrite. Body publication precedes
the content-type update, and header failure still attempts original-body
rollback. It authorizes no response aggregation or general failure-policy
abstraction. Existing storage and materialization contracts remain in
[lazy materialization](../regression-guards/request-body-gate-lazy-materialization.md)
and [mutation physical caps](../regression-guards/request-body-mutation-physical-cap.md).

## Evidence and verification limits

Source: `edgion-gateway/src/plugins/runtime/body_gate.rs`,
`edgion-gateway/src/plugins/http/form_json_transform/plugin.rs`, and
`edgion-gateway/src/plugins/http/json_schema_validation/plugin.rs`.

Canonical coverage in `edgion-gateway/tests/unit/plugins/runtime/body_gate_test.rs`
includes strict physical-cap and generation-conflict diagnostics and direct
`NotBuffered` zero-record behavior. Transformer coverage in
`edgion-gateway/tests/unit/plugins/http/form_json_transform/plugin_test.rs`
checks length/read failures, zero additional refused-read diagnostics, exactly
one overwrite diagnostic and ordered failed rollback retaining 500. Existing
successful publication and rollback tests remain.

Fresh verification on 2026-10-12 passed 54 focused Gateway library tests:
FormJsonTransform (14), body gate (31) and JsonSchemaValidation (9), with zero
failures or ignored tests. Workspace all-targets check and Clippy passed with
no warnings, along with formatting, agent-doc consistency, SSA force, metrics
inventory, Pingora source policy, Gateway API transform contract, unit-test
layout and Gateway no-tracing guards. No full `cargo test --all` or integration
topology ran. These results establish the scoped change, not every request-body
behavior. Independent final-review outcome is recorded in
`tasks/pending/architecture-form-body-error-classification/architecture-form-body-error-classification.md`.

Re-evaluate if error variants or diagnostic meanings change, the adapter stops
recording precise refusals, consumers require different diagnostic semantics,
or response/rollback policy changes. This decision does not suppress defects
in storage, capture admission or consumer HTTP policies.

## Resolution reference

- ARCH-029: bounded AIMPL-C candidate approved on 2026-10-12.
