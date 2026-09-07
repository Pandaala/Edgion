---
name: review-acme
description: Review knowledge for acme, grouped by decision type.
---

# acme review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [ACME service.rs Shape — Audited Tradeoff](intentional-design/acme-service-shape.md)

## Regression guards

- [ACME dns-01 `propagationCheckInterval` — Removed as Dead Knob, Then Re-introduced WITH Real Polling](regression-guards/acme-propagation-check-interval-removed.md)
- [fr-rhttp-01 — ACME HTTP-01 challenge host port-stripping preserves IPv6-literal brackets](regression-guards/challenge-host-split-ipv6-literal.md)
- [Resource change notification backpressure](regression-guards/resource-change-notification-backpressure.md)
- [Resource change notification gating](regression-guards/resource-change-notification-gating.md)
