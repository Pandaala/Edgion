---
name: review-request-mirror
description: Review knowledge for request-mirror, grouped by decision type.
---

# request-mirror review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [RequestMirror Tuning Moved to Annotations — Vendored CRD Prunes Extension Fields — FIXED](intentional-design/mirror-tuning-annotations-vendored-crd-prune.md)
- [Request Mirroring Uses a Separate reqwest Client (Bypasses LB Policy + BackendTLSPolicy) — DESIGN-TRADEOFF (accepted)](intentional-design/mirror-uses-separate-http-client-bypasses-lb-tls-policy.md)

## Regression guards

- [BODY-014 — RequestMirror Uses the Committed Body Snapshot — FIXED](regression-guards/request-mirror-committed-body-snapshot.md)
