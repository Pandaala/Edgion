# Define BackendRef-Scoped Native CORS Preflight Ownership

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Define one explicit policy for native Gateway API CORS attached to backend references so preflight and actual-response behavior cannot become partially active or depend on an unreachable post-selection runtime.

## Requirements

- Decide whether backendRef-scoped native CORS is rejected or receives deterministic preflight selection and application semantics.
- Keep rule-level native CORS ownership and weighted backend behavior coherent; mixed backend policies must never select an implicit or nondeterministic preflight contract.
- Align Controller validation/status, route compilation, backend pinning, Gateway execution order, and supported-feature documentation with the chosen policy.
- Never silently return the generic headerless preflight response when a supported native CORS policy should own the request.
- Preserve canonical local-response diagnostics, access-log ownership, response streaming, and immutable publication.
- Do not modify existing integration-test files.

## Acceptance criteria

- One documented policy covers rule-only, backend-only, and mixed rule/backend or weighted-backend configurations.
- Unsupported configurations fail consistently before activation; supported configurations apply the same effective policy to preflight and actual responses.
- Focused unit tests cover selection, denied origins, and fallback/error paths.
- New standalone cross-process coverage proves backend-only and mixed-backend preflight plus actual-response behavior without rewriting existing integration tests.
