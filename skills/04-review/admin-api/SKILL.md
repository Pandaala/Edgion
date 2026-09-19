---
name: review-admin-api
description: Review knowledge for admin-api, grouped by decision type.
---

# admin-api review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Gateway admin `/configclient` secret redaction](mandatory-design/gateway-admin-configclient-redaction.md)

## Intentional design

- [The Controller Admin API's Write Precondition Is Opt-In — ACCEPTED-TRADEOFF (documented, not enforced)](intentional-design/admin-api-write-precondition-opt-in.md)
- [Admin API CORS: Same-Origin Deployment, No CORS Layer](intentional-design/admin-cors-same-origin-rule.md)
- [Admin REST API RFC-compliance findings are out of scope](intentional-design/admin-rest-rfc-compliance-out-of-scope.md)
- [Controller RegionRoute effective + failover endpoints are retained for single-Controller drill-in — NOT DEAD CODE](intentional-design/controller-region-route-endpoints-retained-for-drill-in.md)
- [Gateway Admin API Response Shape — Accepted Tradeoff](intentional-design/gateway-admin-api-response-shape.md)

## Regression guards

- [Gateway Admin Bearer Authentication Boundary](regression-guards/gateway-admin-bearer-boundary.md)
- [gateway-admin-harden Task 2 — Gateway admin API defaults to loopback bind](regression-guards/gateway-admin-loopback-default.md)
- [Controller Admin API update_namespaced / update_cluster Trusts Body metadata.name](regression-guards/update-namespaced-reject-body-name-mismatch.md)

## Superseded decisions

- [Removed local auth cookie](superseded/removed-local-auth-cookie.md)
- [Removed skip paths superuser bypass](superseded/removed-skip-paths-superuser-bypass.md)
