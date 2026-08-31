# Harden Auth Challenge Realm Quoted Strings

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | bugfix |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Make configurable authentication challenge realms safe and consistent as HTTP quoted-string values through the canonical auth error helper and shared resource validation.

## Requirements

- Inventory every caller of `send_gateway_auth_error_response` and every realm-bearing auth configuration.
- Choose one documented policy for quote and backslash escaping plus control-character rejection, consistent with HTTP quoted-string syntax.
- Apply the policy in the canonical helper and mirror it in shared schema/admission validation so invalid configuration cannot reach runtime.
- Keep error descriptions, challenge scheme selection, status/body, failure delays, and access-log diagnostics unchanged.
- Do not add plugin-local realm escaping or include realm contents in logs/errors.

## Acceptance criteria

- Quotes and backslashes are escaped or rejected consistently according to the documented shared policy.
- CR, LF, NUL, and other forbidden control characters are rejected before response construction.
- Every realm-bearing auth plugin produces a syntactically valid challenge or fails configuration closed.
- Focused shared-helper/config-validation tests, workspace check, Clippy, and repository guards pass.
