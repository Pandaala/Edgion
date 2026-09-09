# Add DSL Request-to-response Context Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | test |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add a new integration scenario proving that a DSL request-stage `ctx.set` value is visible to response-stage DSL execution for the same proxied request.

## Requirements

- Add a new scenario rather than rewriting an existing integration test.
- Configure a request-stage DSL plugin to set a bounded non-sensitive context value and a response-stage DSL plugin to read it and produce a deterministic observable result.
- Prove same-request phase continuity through the real Pingora session/context lifecycle; do not substitute a mock-injected pre-existing value.
- Keep the scenario independent of access-log projection unless that behavior is explicitly configured and asserted with canonical redaction rules.
- Include positive visibility and request isolation so a later request cannot observe the previous request's value.
- Preserve streaming response handling and avoid response aggregation.

## Scope

### In scope

- A new DSL integration scenario and only the minimal fixture/registration support it requires.

### Out of scope

- Changes to existing integration scenarios, production context storage, access-log schema, and unrelated DSL features.

## Acceptance criteria

- The new scenario fails if request- and response-stage DSL plugins do not share the same request context.
- A subsequent request starts with isolated context state.
- Targeted integration execution passes without changing existing scenarios.
