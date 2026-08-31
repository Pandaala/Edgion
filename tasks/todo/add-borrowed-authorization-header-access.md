# Add Borrowed Authorization Header Access

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Provide a canonical borrowed or raw-header access path for authentication consumers so reading Authorization does not first allocate an owned reversible credential String. Preserve object safety, mocks, exact header validation, and every plugin's failure behavior.

## Requirements

- Design one PluginSession header API that can return validated borrowed bytes or a request-scoped view without exposing invalid lifetimes.
- Update the PluginSession boundary, Pingora adapter, mock sessions, and every affected Authorization-reading auth consumer together; do not add a BasicAuth-only bypass or require unrelated header consumers to migrate without independent evidence.
- Keep raw credentials inside the shortest request-scoped lifetime and avoid UTF-8/String construction until a consumer actually requires it.
- Preserve duplicate-header policy, case-insensitive header names, invalid-byte behavior, object safety, and async call boundaries.
- Do not retain borrowed header views across session mutation or await points unless the API proves their stability.
- Preserve sensitive-data redaction and do not log Authorization values.

## Acceptance criteria

- Auth consumers that only parse Authorization no longer require an initial owned String copy.
- Compile-time/API tests prove views cannot outlive or cross unsafe session mutation boundaries.
- Existing auth parsing, failure, delay, header-mutation, and integration behavior remains unchanged.
- Workspace check, Clippy, repository guards, and focused auth/session tests pass.
