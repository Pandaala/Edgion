# HTTP Route Named-Parameter Precedence

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Decide and implement HTTP route precedence that is independent of named-parameter identifier spelling and length. The current radix priority includes every character in the normalized pattern, so a longer parameter name can outweigh a more specific static path even though the identifier does not affect match eligibility.

## Requirements

- Define precedence for overlapping static and named-parameter paths without using parameter identifier length as specificity.
- Preserve the Gateway API Exact-versus-Prefix ordering and Edgion's documented header, query, method, and stable tie-break chain.
- Cover escaped `::` literals, simultaneous overlapping candidates, and names containing different Unicode character counts.
- Verify the complete specificity chain rather than changing only one arithmetic term.

## Scope

### In scope

- `RadixPath` priority modeling and the precomputed candidate-rank build.
- Static-character or segment-specificity semantics for named parameters.
- Focused unit tests for static versus parameter and Exact versus Prefix overlap.
- A later integration scenario that validates externally observable route selection.

### Out of scope

- The allocation-only B0172 route-table build optimization.
- Changes to existing `edgion-tests/**` files during the Gateway quality campaign.
- Regex-route precedence or unrelated HTTP match predicates.

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-gateway/src/routes/http/match_engine/radix_path.rs` | Model path specificity independently of identifier spelling. |
| `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs` | Preserve deterministic rank construction across overlapping candidates. |
| HTTP route tests | Prove the full externally observable precedence chain. |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Defer from B0172 | Correcting this changes observable routing and requires an explicit precedence design; the current campaign keeps existing integration tests frozen. |
