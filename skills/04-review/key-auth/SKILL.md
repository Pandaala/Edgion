---
name: review-key-auth
description: Review knowledge for key-auth, grouped by decision type.
---

# key-auth review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Cross-group duplicate identities are first-wins (KeyAuth pilot — canonical)](mandatory-design/dup-key-first-wins.md)
- [KeyAuth — group name appears in OK access log, not in failures](mandatory-design/group-in-ok-log.md)

## Intentional design

- [`extract_first` / `extract_key` duplication — DESIGN-TRADEOFF (not consolidated)](intentional-design/extract-first-not-shared.md)

## Regression guards

- [`key_auth` `hideCredentials` query/cookie no-op — FIXED (fr-auth2-02)](regression-guards/hide-credentials-query-cookie-strip.md)
- [Auth Query Stripping Preserves an Unmatched Query — FIXED](regression-guards/strip-query-param-no-match-preserves-query.md)
