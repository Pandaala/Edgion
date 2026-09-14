---
name: review-jwe-decrypt
description: Review knowledge for jwe-decrypt, grouped by decision type.
---

# jwe-decrypt review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [JWE Allowed-Algorithm Runtime Check — ACCEPTED-TRADEOFF](intentional-design/allowed-algorithms-defensive-check.md)

## Regression guards

- [JWE resolved-credential failures remain authentication failures](regression-guards/resolved-credential-failure-policy.md)
- [JWE protected headers use strict object parsing and raw AAD](regression-guards/strict-protected-header-json.md)
