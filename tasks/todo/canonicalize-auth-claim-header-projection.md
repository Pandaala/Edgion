# Canonicalize Auth Claim-to-Header Projection

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | design-correctness |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Compile JwtAuth and OpenID Connect claim-to-header mappings into one deterministic, destination-owned projection plan before the request path.

## Requirements

- Define stable projection order before runtime publication; do not sort or normalize on every request.
- Normalize destination header names case-insensitively and reject or otherwise explicitly resolve duplicate ownership.
- Preserve the existing per-value and aggregate byte caps with a deterministic accepted set.
- Reuse one shared plan and validation contract for JwtAuth and OpenID Connect.
- Preserve the existing overwrite-written-names contract: writing a projected header replaces that destination's prior values, but a skipped write does not authorize clearing unrelated headers. If configured claim destinations must also be cleared on skip, specify and test that change locally in JwtAuth/OIDC projection rather than broadening generic header mutation.
- Do not modify existing integration tests.

## Acceptance criteria

- Repeated construction and execution produce identical header writes regardless of source HashMap iteration order.
- Duplicate case variants and aggregate-cap boundaries have explicit resource and runtime tests.
- Gateway format/check/Clippy and repository guards pass.
