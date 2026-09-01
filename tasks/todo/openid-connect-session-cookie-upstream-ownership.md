# Decide OpenID Connect Session-Cookie Upstream Ownership

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-01 |
| Status | pending |
| Type | security |
| Priority | P1 |
| Issue | N/A |
| Depends on | B0093 |

## AI guide summary

Define whether the Gateway-owned OpenID Connect session and state cookies may reach upstream applications, then enforce one documented hiding policy without damaging unrelated Cookie fields.

## Requirements

- Decide whether `edgion_oidc_session` is always internal, follows `hideCredentials`, or is intentionally application-visible; document the security and compatibility tradeoff before implementation.
- Include `edgion_oidc_state` in the ownership analysis even though normal callback processing clears it, so exceptional and pass-through paths have an explicit contract.
- Audit session, bearer-only, callback, logout, `UnauthAction::Pass`, refresh failure, and successful authentication paths.
- Reuse the canonical `auth_common::strip_cookie` mechanism; do not add an OIDC-local Cookie parser.
- Preserve unrelated cookies, multiple Cookie header lines, header ordering rules, session verification, refresh, logout, challenge, failure delay, Dye, and PluginLog behavior.
- Keep credentials, cookie values, claims, provider endpoints, and parser detail out of logs and public error bodies.
- Update the OpenID Connect configuration documentation and any `hideCredentials` wording atomically with the approved contract.

## Acceptance criteria

- Every request path follows the approved session/state-cookie ownership policy before proxying upstream.
- Tests cover multiple Cookie lines, unrelated application cookies, session and state names, bearer and session modes, `hideCredentials` settings if applicable, pass/fail branches, and callback/logout cleanup.
- No new parser, request-level tracing, metric, sensitive log, or error detail is introduced.
- Existing integration tests remain unchanged unless a later approved design proves a real-proxy interaction cannot be verified with colocated tests.
- Workspace format/check/Clippy and repository guards pass.

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-09-01 | Defer the upstream cookie contract from B0093. | The current behavior exposes a replayable Gateway credential upstream, but choosing the removal policy changes an external request-wire contract and requires an explicit ownership decision. |
