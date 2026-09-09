# Add DSL Response-stage HTTP-call Timeout Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Add a new integration scenario proving that response-stage DSL `http.call` can suspend through the synchronous Pingora bridge and enforces the configured timeout clamp.

## Requirements

- Add a new DSL `responsePlugins` scenario rather than rewriting an existing integration test.
- Call an existing deterministic slow test-server endpoint from the response-stage script.
- Configure `httpMaxTimeoutMs` to a short value such as 10 ms while requesting a materially larger `http.call` timeout such as 3,000 ms.
- When `http.call` returns the bounded failure map with `status == 0`, set a fixed client-visible response header; make the nonzero branch observably different.
- Assert that the proxied response remains valid and carries the expected status-zero branch header.
- The scenario must jointly prove that the async outbound future can suspend and complete through `block_in_place + Handle::block_on`, and that the effective timeout is `min(requested timeout, httpMaxTimeoutMs)`.
- Reuse existing test topology and endpoints; do not add a production hook or a unit-test HTTP server.
- Preserve generic failure output, egress controls, response streaming, and canonical outbound-client ownership.

## Scope

### In scope

- One new integration scenario and only the minimal new fixture/registration support it requires.

### Out of scope

- Changes to existing integration scenarios, production bridge/client code, timeout schema/defaults, and response aggregation.

## Acceptance criteria

- The new scenario fails if response-stage async `http.call` cannot complete through the synchronous hook.
- It also fails if the requested 3,000 ms timeout is used instead of the configured short cap.
- The client observes the fixed status-zero response header and a valid proxied response.
- Targeted integration execution passes without modifying existing scenarios.
