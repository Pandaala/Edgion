---
name: review-forward-auth
description: Review knowledge for forward-auth, grouped by decision type.
---

# forward-auth review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [ForwardAuth Default Forwards All Request Headers: Design Tradeoff, Not Vulnerability](intentional-design/default-header-forwarding.md)

## Regression guards

- [ForwardAuth Deny Response Content-Type](regression-guards/deny-response-content-type-preserved.md)
- [`forwardAll` Withholds `Expect` Even Though It Is End-to-End — FIXED](regression-guards/outbound-expect-continue-not-relayed.md)
- [Outbound Subrequest Framing Is Owned by Its HTTP Hop — FIXED](regression-guards/outbound-subrequest-framing-owned-by-hop.md)
- [ForwardAuth Request Body Sources Are Mutually Exclusive — FIXED](regression-guards/request-body-sources-mutually-exclusive.md)
- [forward_auth::send_error_response Swallows insert_header Errors (P3-g3-016)](regression-guards/send-error-response-header-insert-plugin-log.md)
