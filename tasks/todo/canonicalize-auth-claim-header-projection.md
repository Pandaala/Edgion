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

Compile JwtAuth, OpenID Connect, and JweDecrypt claim/payload-to-header mappings into one deterministic, destination-owned projection plan before the request path.

## Requirements

- Define stable projection order before runtime publication; do not sort or normalize on every request.
- Normalize destination header names case-insensitively and reject or otherwise explicitly resolve duplicate ownership.
- Preserve the existing per-value and aggregate byte caps with a deterministic accepted set.
- Include JwtAuth `claimsToHeaders`, OpenID Connect `claimsToHeaders`, and JweDecrypt `payloadToHeaders` in one shared plan and validation contract.
- Expand scalar and array values directly into one bounded candidate. Enforce the per-value cap and remaining aggregate bytes before append or allocation, and commit aggregate budget only after a valid successful write; do not collect array elements into an intermediate `Vec<String>` and then join them.
- Preserve array element order, comma formatting, scalar inclusion rules, UTF-8 byte accounting, missing/empty/unsafe skip semantics, sensitive `HeaderValue` marking, and replace-all-lines destination writes.
- Apply deterministic ordering, case-insensitive destination ownership, and aggregate-cap selection equally to all three consumers.
- Preserve the existing overwrite-written-names contract: writing a projected header replaces that destination's prior values, but a skipped write does not authorize clearing unrelated headers. If configured claim destinations must also be cleared on skip, specify and test that change locally in JwtAuth/OIDC projection rather than broadening generic header mutation.
- Do not modify existing integration tests.

## Acceptance criteria

- Repeated construction and execution produce identical header writes regardless of source HashMap iteration order.
- Repeated shuffled construction, duplicate case variants, and exact/plus-one per-value and aggregate boundaries have explicit resource and runtime tests across all three consumers.
- Array projection tests prove bounded early stop without full intermediate materialization while preserving mixed-scalar filtering, order and formatting.
- JweDecrypt `payloadToHeaders` has the same deterministic-order, duplicate-destination, sensitive-header and exact-cap coverage as JWT and OpenID Connect.
- Gateway format/check/Clippy and repository guards pass.
