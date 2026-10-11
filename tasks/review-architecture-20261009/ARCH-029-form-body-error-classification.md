---
id: ARCH-029
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-312
independent_verification: confirmed by /root/verify_ip_union
---

# Reuse the canonical body-error access-status classifier in FormJsonTransform

## Current ownership and change cost

`FormJsonTransform::record_body_error` in
`edgion-gateway/src/plugins/http/form_json_transform/plugin.rs:69` repeats the
complete `BodyReadError` to `EdgionErrorCode` classification already owned by
`plugins/runtime/body_gate.rs::record_body_read_error_status` at line 115.
Both map size failures, timeout, incomplete capture, storage failure, generation
conflict and unimplemented reads identically, and intentionally leave
`NotBuffered` alone. FormJsonTransform invokes its copy for length, read,
overwrite and rollback errors. JsonSchemaValidation already uses the shared
helper for the equivalent observability signal.

An added body-error variant or a changed canonical access-log status requires
updating the runtime classifier and this plugin copy together. Exhaustive
matches help find added variants; they do not ensure matching status choices
when an existing variant changes meaning. This is a second implementation of
one diagnostic mechanism, rather than evidence of a current wrong response.
The cost is a redundant semantic maintenance obligation, not measured runtime
overhead or a need for a generic plugin failure-policy abstraction.

## Bounded improvement and alternative

Import and call the existing `record_body_read_error_status` helper at the four
FormJsonTransform sites, then remove its private classifier. Keep HTTP status
selection, static replies and PluginLog messages in the transformer. Its 413
mapping for size failures and 500 mapping for other failures belong to its
operation policy and need not match JsonSchemaValidation's read-error replies.

Keeping the local classifier with parity tests is possible but preserves the
second owner and tests implementation duplication. Reuse requires no new trait,
allocation, context accessor, public schema field or factory. If ARCH-002 later
relocates body contracts, update this consumer with the other callers instead of
recreating a plugin-local mapping.

## Required invariants and future verification

- Preserve the adapter's more precise refusal status for `NotBuffered`.
- Record every existing mapped error once at the same point before the same
  fixed reply; add no tracing, metrics or dependency detail.
- Preserve media/length checks before materialization, canonical complete
  request BodyStorage ownership, output admission and mutation atomicity.
- Preserve body-before-content-type publication and rollback on header failure,
  including the original selected 500 when rollback itself fails.
- Use existing storage-failure and overwrite/rollback regression bodies to
  check wiring; a future implementation should also inspect canonical classifier
  coverage for all variants rather than duplicate the match in a new test.

## Original evidence and limits

Read the graded canonical-mechanism and observability rules, original
`request-body-gate-lazy-materialization.md` and
`request-body-mutation-physical-cap.md`, actual runtime body gate, factory and
adapter seams, and selected FormJsonTransform/JsonSchemaValidation tests.
Those contracts justify independent operation policies and a single shared
request-body owner; they do not require a private duplicate access-status map.
ARCH-002 concerns body contract dependency direction and does not already
remove this second classifier. No code changes, tests or builds were executed.
Independent verification remains pending.
