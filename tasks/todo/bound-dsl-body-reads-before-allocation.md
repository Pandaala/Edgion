# Bound DSL Body Reads Before Allocation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Enforce the DSL string limit while reading request-body text so range reads and lossy UTF-8 conversion cannot allocate a result beyond `maxStringLen` before rejecting it.

## Requirements

- Trace `BodyRead` and `BodyReadAll` through the canonical request-scoped `BodyStorage`, including memory-backed and spill-backed bodies.
- Provide one bounded text collection path for the requested byte range that checks the exact lossy UTF-8 output length before each admitted append or allocation request.
- Reject the first output byte beyond `VmLimits::max_string_len` with the existing typed `StringTooLong` behavior; do not return a truncated value.
- Preserve byte-range semantics, missing/body-read errors, scan-budget charging and refunds, request-only body caching, and response-stage access to the already captured request body.
- Do not introduce another body cache, response aggregation, request-path tracing, logs, metrics, or sensitive body diagnostics.
- Add focused unit coverage for in-memory and spilled bodies, valid and invalid UTF-8, empty and partial ranges, exact-limit and plus-one output, and scan-budget refund behavior.
- Do not modify existing `edgion-tests/**` files. If end-to-end coverage later proves necessary, record it as a separate issue.

## Scope

### In scope

- DSL body read builtins, the narrow canonical `BodyStorage` API needed for bounded text collection, focused crate-local tests, and directly owning security documentation.

### Out of scope

- Response-body buffering, body-policy schema changes, unrelated DSL string operations, and integration-test rewrites.

## Acceptance criteria

- Neither `BodyRead` nor `BodyReadAll` allocates a raw or lossy text result beyond the admitted DSL string limit before reporting overflow.
- Existing range, storage, stage, error-policy, and budget semantics remain unchanged.
- Focused memory/spill/UTF-8 boundary tests and all repository guards pass.
