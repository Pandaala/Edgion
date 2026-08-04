# Subtasks

## Design

- [x] Inspect AI-1a resource, request, stream-processor, metering, and integration seams.
- [x] Verify the current Anthropic Messages/SSE/error contract from official sources.
- [x] Freeze first-slice scope and strict unsupported-field policy.
- [x] Complete independent design review and record adopted decisions.

## Implementation

- [x] Resource schema, CRD, runtime compilation, and enum drift guard.
- [x] Pure OpenAI-to-Anthropic request converter and fixed local errors.
- [x] Response header normalization and fixed provider-error mapping.
- [x] Bounded non-streaming and SSE response translators.
- [x] AiProxy provider dispatch and processor ordering.
- [x] Anthropic usage label, metric ceiling, access-log validation, and inventory docs.
- [x] Direct unit tests.

## Verification

- [x] Independent code review and resolution.
- [x] Static Anthropic integration suite.
- [x] Reuse and verify the shared dynamic Secret rotation scenario.
- [x] Preserve and rerun AI-1a targeted coverage.
- [x] Run required quick checks and repository guards.
- [x] Update implementation record, decisions, and wrap-up.
