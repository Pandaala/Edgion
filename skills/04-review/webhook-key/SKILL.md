---
name: review-webhook-key
description: Review knowledge for webhook-key, grouped by decision type.
---

# webhook-key review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [WebhookKeyGet + LinkSys Resolver Has No Response Signature Verification: Relying on TLS is the Industry Pattern](intentional-design/response-signature-not-needed.md)

## Regression guards

- [LinkSys webhook-auth WebhookAuth::Hmac Silent No-op in inject_auth](regression-guards/webhook-auth-hmac-fail-closed-upsert-guard.md)
