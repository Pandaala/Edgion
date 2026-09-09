# Bound DSL Runtime-limit Scalars

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define and enforce one resource-owned range contract for every DSL runtime-limit scalar so Controller validation and standalone Gateway construction cannot publish effectively unbounded work.

## Requirements

- Inventory `maxListLen`, `maxMapSize`, `httpMaxTimeoutMs`, `httpMaxRequestBodyBytes`, `httpMaxResponseBodyBytes`, and every other DSL runtime-limit scalar across resource validation, Gateway construction, builtins, documentation, and review precedent.
- Establish explicit lower/upper ranges and intentionally preserve or change zero semantics with evidence. Reuse canonical limits such as `MAX_OUTBOUND_TIMEOUT_MS = 60_000` when their semantics match; do not introduce a competing timeout policy.
- Move all scalar checks into one resource-owned validation path consumed by both `validation_error()` and `compile_and_validate()`, preserving deterministic error ordering and Controller/Gateway parity.
- Prevent an operator value such as `u64::MAX` from becoming an effectively unbounded `http.call` deadline, especially while response-stage execution waits through the accepted `block_in_place + block_on` bridge.
- Bound collection/output configurations before they authorize large split lists, maps, Redis `HGETALL` projections, or outbound response retention. Preserve stricter downstream physical/string/body caps.
- Add exact minimum/maximum, zero, plus-one, overflow, and dual-fault ordering tests plus standalone request/response factory coverage.
- Update the English and Chinese DSL configuration/security documentation together.
- Do not modify existing `edgion-tests/**` files. Track any later justified end-to-end coverage separately.

## Scope

### In scope

- DSL resource-owned scalar validation, Gateway construction parity, directly affected builtins/runtime limits, focused unit tests, and bilingual DSL documentation.

### Out of scope

- New failure-policy shapes, response buffering, outbound-client redesign, Tokio runtime redesign, or unrelated plugin schemas.

## Acceptance criteria

- Every DSL runtime-limit scalar has one documented and tested resource-owned range contract used by Controller validation and all Gateway factories.
- Response-stage outbound calls cannot receive an effectively unbounded configured timeout.
- Existing valid configurations retain their behavior unless an explicitly justified range decision says otherwise.
- Focused tests and all repository guards pass.
