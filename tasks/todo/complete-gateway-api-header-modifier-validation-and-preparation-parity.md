# Complete Gateway API Header-Modifier Validation and Preparation Parity

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Validate every native and EdgionPlugins HTTP/GRPC header modifier before publication and prepare one typed immutable runtime representation so malformed configuration never becomes a request-time surprise.

## Requirements

- Canonically validate `HTTPHeaderFilter` and `GRPCHeaderFilter` names, values, list bounds, type/config pairing, and repeated single-instance filters against the applicable Gateway API v1.5 contract.
- Cover HTTP and GRPC rule and backend filter lists, including delegated or effective rules and the EdgionPlugins preserve-unknown-fields path.
- Keep Controller invalid-rule/status and Gateway defensive-construction behavior consistent for every ingest backend.
- Prepare typed immutable header names and values once when publication succeeds; retain first-equivalent-entry and set-then-add-then-remove semantics.
- Coordinate response-field restrictions with D0068 and request framing with D0091 instead of adding local deny-lists.
- Keep errors static and non-sensitive, use canonical access-log diagnostics, add no tracing, and preserve response streaming.
- Keep English and Chinese header-modifier documentation synchronized.
- Do not modify existing integration-test files.

## Acceptance criteria

- Shared-resource tests cover all structural and header parse failures plus valid boundary values.
- Controller tests cover HTTP/GRPC rule and backend invalidation with valid sibling rules retained.
- Gateway tests prove defensive fail-close construction and allocation-free typed execution.
- A standalone integration issue is added only if unit tests cannot prove the ingest boundary.
