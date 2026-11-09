---
name: review-ldap-auth
description: Review knowledge for ldap-auth, grouped by decision type.
---

# ldap-auth review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [LDAP Auth Per-Request Connection — Accepted with Operational Bounds](intentional-design/per-request-connection-accepted.md)

## Regression guards

- [LDAP Auth Bounded Response Parsing — FIXED (auth-01, auth-02)](regression-guards/bounded-response-parsing.md)
- [LDAP Auth Dial-time Egress Policy and TLS Integration Coverage](regression-guards/dial-time-egress-policy.md)

## Review evidence

- [LDAP Auth Discriminated Error Log Tags — NOT-AN-ISSUE (security) + convention alignment (fixed F-006, 2026-05-26)](review-evidence/discriminated-error-log-tags.md)
