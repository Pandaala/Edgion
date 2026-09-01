# Define Complete RegionRoute Table Validation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | architecture-correctness |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0099 |

## AI guide summary

Design one canonical validator for RegionRoute base, overlay, and effective tables before tightening endpoint, range, override, or failover semantics.

## Requirements

- Inventory resources validation, Controller/Gateway preparse, region overlay publication, management APIs, request selection, failover execution, outbound peer security, and both documentation sets.
- Define strict endpoint syntax and normalization without bypassing canonical DNS and post-resolution unsafe-address checks.
- Define hash-range bounds, inclusive/exclusive endpoints, overlap handling, gaps, and precedence.
- Define when an empty base table is valid with or without `overrideRef` and how absent or stale overlays behave.
- Define dangling, self-referential, and cyclic failover behavior and make it consistent across admission, effective generations, and runtime fallback.
- Implement one shared validator and update every producer/consumer atomically; do not add Gateway-only defensive semantics as the primary contract.
- Keep existing integration tests frozen unless the approved design changes observable hot-reload or failover behavior; create a separate integration issue if needed.

## Acceptance criteria

- A written decision matrix covers every endpoint, range, overlay, and failover case before implementation.
- Resources, Controller, Gateway, and management API outcomes share the same validation result.
- Deterministic tests cover base-only, overlay-only, replacement, stale/missing overlay, gaps/overlaps, and failover graphs.
- Documentation matches the implemented contract in English and Chinese.
- Workspace format/check/Clippy and repository guards pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Defer broad validation from B0099. | The immediate modulo-zero panic has a narrow canonical fix; full table admission changes operator and overlay semantics and needs an explicit cross-component design. |
