---
name: admin-rest-rfc-compliance-out-of-scope
description: Use when a reviewer files an RFC 9110/9111 (or REST-convention) compliance finding against a Controller Admin REST endpoint under `/api/v1/...` — these are out of scope for this review process; only data-plane gateway RFC compliance is tracked here.
knowledge_type: intentional-design
---

# Admin REST API RFC-compliance findings are out of scope

**Status / verdict**: out of scope (review-process scope boundary)

The in-repo Controller Admin REST API under `/api/v1/...` is **out of scope**
for this RFC review process. Only data-plane gateway RFC compliance is tracked
here. EdgionCenter is maintained in a separate repository and must be reviewed
against that repository's own workflow.

Admin endpoints exist for operator tooling (edgion-cli, dashboard, ops scripts) and are not subject to public HTTP-protocol conformance review. They are an internal control-plane surface consumed by trusted first-party clients, not a public protocol endpoint that third parties code against, so the cost of strict RFC/REST conformance is not justified by the operator-tooling use case.

Reject re-reports of the following forms:

- "admin endpoint returns 200 instead of 201/204/4xx per RFC 9110/9111"
- "admin response is missing `Retry-After` / `Cache-Control` / `Location` / `WWW-Authenticate`"
- "admin error mapping doesn't follow REST conventions"
- any naming finding scoped to admin REST types (e.g. `ApiResponse`)

Before re-reporting any RFC/REST-conformance finding, verify the endpoint in question is a **data-plane gateway** endpoint and not an admin `/api/v1/...` endpoint. If it is an admin endpoint, the finding is out of scope and is not accepted.
