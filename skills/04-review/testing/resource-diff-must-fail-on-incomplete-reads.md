---
name: resource-diff-must-fail-on-incomplete-reads
description: Use when reviewing integration resource synchronization validation and its error handling.
---

# Resource diff must fail on incomplete reads

The integration `resource_diff` command proves Controller-to-Gateway synchronization only when
every requested resource list was fetched and deserialized completely. Authentication, transport,
HTTP, API-envelope, or individual resource decode failures are infrastructure errors and must exit
with code 2. They are not equivalent to a successfully fetched empty resource list.

Controller resource requests use the `EDGION_ADMIN_TOKEN` bearer token and Gateway resource
requests use `EDGION_GATEWAY_ADMIN_TOKEN`. Probe health remains unauthenticated on the dedicated
probe ports. Secret-reference validation follows the same fail-closed read policy as the primary
resource comparison. Gateway is namespace-scoped: list it through `/api/v1/namespaced/Gateway`,
and resolve an unqualified listener `certificateRef` relative to that Gateway's namespace.

Regression coverage must include a rejected Controller token, propagation of a failed kind read,
the namespaced Gateway list endpoint, and a malformed item within an otherwise successful list response.
