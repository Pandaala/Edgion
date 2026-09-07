# Decide Credentialed Native CORS Expose-Headers Wildcard Semantics

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Choose and enforce a version-pinned Gateway API and Fetch contract for `exposeHeaders: ["*"]` when credentials are allowed.

## Requirements

- Record the applicable Gateway API v1.5 and Fetch interpretation before selecting reject, omit, literal, or explicit-list behavior.
- Do not infer browser credentials mode solely from Cookie or Authorization header presence.
- Align shared validation, Controller status, Gateway runtime output, supported-feature claims, and user documentation with the chosen behavior.
- Preserve existing wildcard behavior for origins, methods, and allowed headers unless separately justified.
- Keep diagnostics bounded and in the canonical access log; add no request-path tracing or parallel observability.
- Do not modify existing integration-test files.

## Acceptance criteria

- The selected behavior is explicit for credentialed and non-credentialed responses and has no browser-dependent hidden branch.
- Invalid configurations fail consistently before activation if rejection is selected.
- Focused validation and runtime tests cover the exact emitted or omitted header behavior.
- Cross-process coverage is added within this task if the decision changes publication or response behavior.
