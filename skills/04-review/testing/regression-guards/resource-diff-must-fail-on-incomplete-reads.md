---
name: resource-diff-must-fail-on-incomplete-reads
description: Use when reviewing integration resource synchronization validation and its error handling.
knowledge_type: regression-guards
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

## Resolution references

- `ITEST-003` — 2026-08-26.

## Native Listener observations (ARCH-339)

The PortConflict suite requires explicit named Listener `Conflicted=False` observations in its
cross-Gateway and different-hostname no-conflict cases. Missing Listener entries, missing
conditions, Unknown and True are not evidence of no conflict. Rust selects the fixture namespace
and name and rejects duplicate Listener or Conflicted entries. The Kubernetes different-hostname
branch requires the existing False predicate for both `api` and `web` while retaining its
unexpected-True checks. The optional Gateway-level `ListenersNotValid` condition may be absent;
its True value still fails these cases.

Synthetic reader and shell checks establish this observation boundary, not live topology health,
condition generation freshness, socket binding, or data-plane publication. The separate internal
conflict admission shortcut is unchanged.
