---
name: jwt-hide-credentials-query-cookie-strip
description: Use when reviewing whether JwtAuth hideCredentials removes query and cookie tokens before upstream forwarding.
status: fixed
finding_id: gw-plugin-auth-a-01
closed: 2026-08-02
knowledge_type: regression-guards
---

# JwtAuth `hideCredentials` query/cookie stripping — fixed

## Conclusion

`JwtAuth` removes the successfully authenticated token from its actual request
source when `hideCredentials: true`: the configured header is removed, the
configured query parameter is stripped from the forwarded query, or the
configured cookie is stripped from all forwarded `Cookie` header lines.

The query and cookie implementations are shared with `KeyAuth` through
`auth_common::{strip_query_param, strip_cookie}`. Re-reports should verify both
plugins still call these helpers before claiming either source is forwarded.

## Required semantics

- Query stripping compares the percent-decoded parameter name but preserves
  every surviving pair verbatim. It updates only the query through
  the canonical structured Query removal operation, leaving raw-vs-normalized path selection
  to the upstream stage.
- Cookie stripping reads every `Cookie` field line, removes every matching
  cookie, and preserves unrelated cookies. This covers HTTP/2 requests where
  cookies may arrive in multiple fields.
- Cookie cleanup is atomic and fallible: invalid UTF-8 in any field during
  removal returns 400; internal mutation failures return 500 through the canonical
  runtime error dispatcher. No later plugin or upstream request runs on failure.
  No-match deletion preserves the raw fields; successful edits update the live
  HeaderMap immediately for webhook, mirror and subsequent plugin readers.
- Only the source that supplied the validated token is removed. A lower-priority
  query or cookie value is not treated as the authenticated credential when a
  header token won extraction priority.
- `hideCredentials: false` leaves the request unchanged.

## Regression coverage

- Shared helper unit tests cover query preservation, full query removal,
  unrelated-cookie preservation, full Cookie-header removal, and multi-line
  Cookie merging.
- The `EdgionPlugins/JwtAuth` integration suite sends authenticated query and
  cookie requests to an upstream echo endpoint and verifies that the JWT is
  absent while an unrelated query parameter or cookie survives.

## Re-evaluation triggers

Re-open if the canonical structured Query removal operation begins finalizing the URI, the
upstream stage stops reading the live rewritten query, or Cookie replacement no
longer removes all old field lines.


## Resolution references

- `gw-plugin-auth-a-01` — 2026-08-02.
