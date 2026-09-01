# Use One EdgionConfigData Snapshot per Authorization Decision

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | concurrency-correctness |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0082 |

## AI guide summary

Make every authorization decision resolve all EdgionConfigData references from one immutable published generation.

## Requirements

- Expose one canonical owned or guarded EdgionConfigData snapshot API; do not add a plugin-local cache.
- Migrate HTTP IpRestriction, stream IpRestriction, TLS-route restriction, and GlobalConnectionIpRestriction together.
- Resolve deny lists, allow lists, Selectors, and every referenced key through the same snapshot for one request or connection decision.
- Preserve live reload between decisions, missing/denied reference failure policy, matcher ordering, and canonical deny-first evaluation.
- Add deterministic generation-swap tests that cannot pass when one decision mixes generations.
- Add no blocking locks, request-path tracing, high-cardinality metrics, or integration-test edits.

## Acceptance criteria

- One authorization decision performs one generation capture regardless of reference count.
- Every consumer uses the shared snapshot mechanism and no direct per-key global load remains in its decision path.
- Unit, compile, lint, and repository guards pass.
