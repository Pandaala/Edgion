# Prepare Gateway API header modifier runtime values

## Meta

- Created: 2026-09-11
- Status: pending
- Type: refactor
- Priority: P2
- Source: complete-gateway-api-header-modifier-validation-and-preparation-parity

## AI guide summary

Recover the deferred follow-up referenced by the
[header modifier validation guard](../../skills/04-review/plugin-runtime/regression-guards/header-modifier-validation.md).
The validation-only correction did not implement typed runtime preparation or
unify AI backend duplicate execution with the ordinary native modifier plan.
This record restores missing tracking; it does not authorize implementation or
claim that the deferred work is fixed or accepted as unnecessary.

## Evidence and scope

- `edgion-gateway/src/plugins/runtime/gateway_api_filters/header_modifier.rs`
  prepares `HeaderModifierPlan` with `Vec<HTTPHeader>` and `Vec<String>` and
  retains the first case-equivalent name within each action list.
- The request and response modifier filters consume those string-backed entries;
  evaluate preparing typed header names/values once before runtime publication.
- `edgion-gateway/src/ai/dispatch.rs` directly applies the backend's configured
  request-header filter entries, including same-list duplicates. Resolve the
  documented parity gap through the canonical preparation mechanism.
- Preserve shared validation, persistent invalid-config rejection, set/add/remove
  order, sensitive metadata where applicable, and append multi-value semantics.
- Request framing and response-field restriction policy remain outside this task.

## Verification required

Revalidate the current consumers before design. Cover HTTP/gRPC request/response
and AI backend preparation parity, case-equivalent duplicates, invalid-generation
rejection/recovery, and typed value preservation. Obtain independent review and
run repository checks before closure. No implementation or runtime test execution
is claimed by this tracking repair.
