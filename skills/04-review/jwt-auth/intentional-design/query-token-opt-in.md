---
name: query-token-opt-in
description: Use when reviewing JwtAuth token extraction or a re-report that "JWT is read from the query parameter with no opt-out". The query path is opt-in via allowTokenInQuery (default false); reject re-reports.
knowledge_type: intentional-design
---

# JwtAuth — query-parameter token is opt-in (allowTokenInQuery, default false)

`extract_token` tries Header > Query > Cookie, but the **query branch is gated**
by `config.allow_token_in_query`, which defaults to `false`. When it is `false`
the `query` field is ignored entirely and a token carried only in the URL is
treated as absent (request is unauthenticated).

RFC 6750 §2.3/§5.3 advise (SHOULD NOT) against bearer tokens in URLs because they
leak into access logs, browser history, and `Referer` headers. Making the path
opt-in satisfies the "explicit opt-in for risky behaviour" principle without
removing the capability for operators who genuinely need it.

## What re-reviews must reject

- "JWT can be read from query with no way to disable it" — **fixed**; the
  `allowTokenInQuery` knob disables it, and it is off by default.
- "Add a per-request `tracing::warn!` when a token comes from the query" —
  **declined**: JwtAuth runs on the data-plane hot path, where `tracing::*` is
  prohibited (`skills/03-coding/01-log-safety.md`). The opt-in flag is the
  surfacing mechanism, not a per-request log.
- "Set `Cache-Control: no-store` from the plugin when a query token is used" —
  **declined**: this is a request-stage plugin (no response handle here), and
  RFC 6750 §2.3's `no-store` SHOULD targets clients, not the resource server.

## Implementation

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/jwt_auth.rs` — `allow_token_in_query: bool` (`#[serde(default)]`, Default = false)
- `edgion-gateway/src/plugins/http/jwt_auth/request_stage.rs` — `extract_token` guards the query branch with `if self.config.allow_token_in_query`
- `edgion-gateway/tests/unit/plugins/http/jwt_auth/mod_test/token_source.rs` — `test_query_token_rejected_when_not_allowed_by_default`

## Borrowed extraction boundary

Token extraction borrows its selected input until synchronous verification
finishes. Header extraction uses the live first HeaderMap value when it is text-eligible;
query extraction uses `get_query_param_ref` and the existing frozen decoded
query cache; Cookie extraction uses the same canonical `cookie_value_ref` parser
as the owned session getter. No new source or cache is introduced.

Priority and transformations remain Header > enabled Query > Cookie. Only the
header is trimmed and accepts the case-insensitive `Bearer ` prefix. In
particular, `Bearer ` becomes the nonempty raw token `Bearer` after overall trim,
so it does not fall through. Empty query/cookie values fall through; their values
are not additionally trimmed. Cookie first-match, key-case and opaque-field
ordering remain canonical, including an empty first match masking later values.
An opaque first Authorization value does not search later duplicates.

`verify_jwt` returns owned claims/issuer and a store-bound group reference, not
a token reference. The session borrow ends before claim propagation, credential
hiding or asynchronous failure handling. Invalid selected tokens still reject
without trying lower-priority sources or allowing anonymous access. Issuer/key
binding, failure logs, delay and challenge behavior remain separate unchanged
contracts.

Extraction tests cover these boundaries and pointer identity with prepared
fixture storage. They establish borrowed selected-value ownership, not zero
allocations across extraction, query-cache initialization, verification or claim
propagation. No throughput claim follows. Existing verification tests retain
algorithm, claim, anonymous and failure-response coverage.

## Operator-visible note

This flipped a default: before, a non-empty `query` (default `"jwt"`) always
accepted query tokens. Deployments relying on `?jwt=<token>` must now set
`allowTokenInQuery: true`. Documented in the en/zh jwt-auth user guides.

## Related

- `skills/04-review/jwt-auth/mandatory-design/group-in-ok-log.md` — JwtAuth access-log conventions
- `skills/03-coding/01-log-safety.md` — data-plane hot-path tracing prohibition

## Resolution references

- `cookie-auth-06` — 2026-05-20.
