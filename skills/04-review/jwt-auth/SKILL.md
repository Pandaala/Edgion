---
name: review-jwt-auth
description: Review knowledge for jwt-auth, grouped by decision type.
---

# jwt-auth review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [JwtAuth — group + iss in OK access log, neither in failures](mandatory-design/group-in-ok-log.md)
- [JwtAuth — iss binds keys (security contract)](mandatory-design/issuer-key-binding.md)

## Intentional design

- [openid_connect vs jwt_auth JWT Helper Split — DESIGN-TRADEOFF](intentional-design/jwt-oidc-helper-split-deliberate.md)
- [JwtAuth — query-parameter token is opt-in (allowTokenInQuery, default false)](intentional-design/query-token-opt-in.md)

## Regression guards

- [Gateway Bearer WWW-Authenticate error / error_description attributes](regression-guards/bearer-www-authenticate-error-attributes.md)
- [JwtAuth `DecodingKey` construction — fixed](regression-guards/decoding-key-prebuilt.md)
- [JwtAuth `hideCredentials` query/cookie stripping — fixed](regression-guards/hide-credentials-query-cookie-strip.md)

## Superseded

- [Historical four-path credential loading](superseded/jwt-auth-credentials-source-paths.md)
- [Historical payload-selector decision](superseded/jwt-auth-multi-key-double-decode.md)
