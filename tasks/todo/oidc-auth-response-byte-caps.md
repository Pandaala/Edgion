# Bound OpenID Connect Authentication Responses

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | security |
| Priority | P1 |
| Issue | N/A |
| Source | D0036 credential ownership audit |

## AI guide summary

Apply hard streaming byte caps to OpenID Connect token exchange and refresh responses, then audit every remaining OIDC HTTP response consumer so no dependency can force unbounded body aggregation.

## Requirements

- Replace direct `reqwest::Response::json` aggregation in authorization-code token exchange and refresh with a cap enforced while streaming.
- Reuse the canonical bounded JSON/body helper when its error and observability contract fits; do not introduce a second plugin-local buffering mechanism.
- Select and document a finite cap for token endpoint responses, including the `Content-Length` fast rejection and chunked/dishonest response path.
- Audit discovery, JWKS, userinfo, revocation, introspection, end-session support, and any other OIDC HTTP response consumers; record which already have a streaming cap, consume no body, or require follow-up.
- Preserve endpoint admission, redirect policy, outbound TLS, deadlines, token endpoint authentication, refresh singleflight/cache semantics, logout revocation behavior, SAN metrics, and auth failure policy.
- Map oversize, body-read, and parse failures to bounded internal classifications and generic public behavior. Keep request-level observability in `PluginLog` and the canonical access log without provider bodies, URLs, tokens, cookies, claims, or credentials.
- Keep this work distinct from public OIDC error-response redaction; do not claim that a byte cap by itself fixes disclosure.
- Do not modify existing integration tests; record any needed new integration scenario separately.

## Scope

### In scope

- OIDC HTTP response body boundedness, starting with token exchange and refresh.
- Focused boundary tests for declared length, chunked bodies, exact-limit success, oversize rejection, read failure, and malformed JSON.

### Out of scope

- Public error-body redesign tracked by `redact-openid-connect-dependency-error-responses.md`.
- Token/cookie ownership, cache TTL, provider protocol, or authentication-policy changes.

## Acceptance criteria

- Token exchange and refresh enforce the selected hard cap during streaming and never call uncapped `Response::json`.
- The OIDC consumer audit accounts for every response body and leaves no unclassified unbounded aggregation path.
- Boundary tests demonstrate bounded handling for honest and chunked oversized responses while valid responses retain existing semantics.
- Public responses and logs contain no dependency body or sensitive detail, and existing failure-policy and observability ownership remain unchanged.
- Gateway format/check/Clippy and documentation, observability, and source-policy guards pass; existing integration tests remain unchanged.
