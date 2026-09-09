# Add Deferred-credential Connection Nomination Integration Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Prove on the real HTTP/1 pipeline that a late `Connection` nomination cannot
cause deferred Secret credentials to be reinserted after Pingora sanitization.

## Requirements

- Install CredentialInjector before a later request-header mutation plugin.
- Have the later plugin nominate the credential target through `Connection`.
- Exercise Pingora's H1 sanitizer and upstream-request hook ordering.
- Assert a fail-closed client response and zero backend receipt.
- Assert the selected endpoint is not penalized as an outlier for the local policy failure.
- Use bounded waits, redacted assertions, and deterministic cleanup.

## Scope

- Add or extend integration coverage only after explicit authorization for the exact integration paths.
- Do not modify existing `edgion-tests/**` files without explicit path-level authorization.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-09 | Schedule a separate repair | Existing integration tests are the frozen validation control line. |
