---
name: review-csrf
description: Review knowledge for csrf, grouped by decision type.
---

# csrf review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [CSRF `expires` Stays Positive Integer Seconds — ACCEPTED-TRADEOFF](intentional-design/csrf-expires-integer-seconds-retained.md)

## Regression guards

- [CSRF cookie `Secure` is operator-configurable — FIXED](regression-guards/cookie-secure-configurable.md)
- [CSRF Secret Rotation Has Shared Ref/Requeue Logic Plus Dedicated Integration Coverage — FIXED](regression-guards/csrf-secret-rotation-integration-coverage.md)
- [Plugins queue Set-Cookie via set_response_header, wiping other Set-Cookie values (gateway-plugins-02)](regression-guards/set-cookie-append.md)
