---
name: controller-region-route-endpoints-retained-for-drill-in
description: Use when reviewing findings that call the Controller's RegionRoute effective/failover endpoints dead code superseded by the generic EdgionConfigData PUT; covers why those two survive while the sibling stubs were deleted.
status: accepted-tradeoff
finding_id: new_work-06
closed: 2026-07-27
---

# Controller RegionRoute effective + failover endpoints are retained for single-Controller drill-in — NOT DEAD CODE

## Conclusion

`GET /api/v1/region-routes/effective` and `POST /api/v1/cluster-region-routes/failover`, the
synthetic `RegionRoute` authorization kind, and `Verb::Failover` are **live and load-bearing**,
even though Center's fleet-wide failover now goes through a generic `EdgionConfigData` PUT with
an `If-Match` CAS precondition. The Center dashboard reaches these two endpoints by drilling
into one Controller (`/controller/{id}/region-routes`) through the federation proxy tunnel.

Fix suggestions of the form "failover is now a generic `EdgionConfigData` PUT, so delete the
Controller's dedicated RegionRoute surface and the synthetic authz kind with it" are **not
accepted**.

## Core Rationale

**1. Two different operations share the word "failover"**

The write-model convergence replaced *fleet-wide* failover — Center fanning one intent out to
every online Controller — with a generic namespaced `EdgionConfigData` PUT + CAS. It did not
replace *per-Controller* failover, where an operator inspects one cluster's effective region
view and flips one region on that Controller alone. The generic PUT cannot serve the read half
at all: `region-routes/effective` merges the `EdgionPlugins.RegionRouteConfig` base table with
the `RegionRouteOverride` `EdgionConfigData` overlay, and that merge is deliberately performed
by the Controller so it stays the single authority for the effective view.

**2. Center does not aggregate this view; it proxies to one Controller**

There is no Center-side region-route map any more (`CenterMetaDataStore.region_routes` and its
readers were deleted). `web/src/pages/RegionRoute/RegionRouteList.tsx` calls
`regionRouteApi.listRegionRoutes()` / `regionRouteFailover()`, which resolve against the
proxy-rewritten base URL `/api/v1/proxy/{controller_id}/api/v1` — so the requests land on the
Controller's own endpoints, not on a Center endpoint. Grepping EdgionCenter for
`/api/v1/center/region-routes` therefore finds nothing and is not evidence that the Controller
surface is unused.

**3. The synthetic authz kind is required by the drift guard, not vestigial**

Both retained routes classify through `(Verb::List, "RegionRoute")` and
`(Verb::Failover, "RegionRoute")`. `authz_classifier.rs`'s `every_admin_route_is_classified`
scrapes the route table via `include_str!`, so removing the kind or the verb while the routes
stay registered reds the test *and* makes both endpoints 403 for every caller including the
wildcard admin token. The synthetic kind is the only authorization shape that does not fit the
standard verb × `ResourceKind` model; that irregularity is accepted for as long as these two
routes exist.

## What WAS deleted (2026-07-27), so re-reports on these are stale

- `GET /api/v1/cluster-region-routes` and `GET /api/v1/service-region-routes` — deprecated
  stubs whose bodies were a literal empty-list response.
- `POST /api/v1/service-region-routes/failover` — a byte-identical delegate to the cluster
  form (same `patch_edgion_config_data_failover`, same `"RegionRouteOverride"` argument).
- `center.ping_interval_secs` — a config field no code ever read; heartbeat cadence is
  Center-driven.

## Fix Suggestions Not Accepted

- "Delete `region_route_handlers.rs` entirely — failover is a generic PUT now" — deletes the
  effective-view merge, which the generic resource API cannot reproduce, and breaks the
  dashboard's Controller drill-in page.
- "Drop `Verb::Failover` / the `RegionRoute` label to make Controller authorization uniform" —
  correct only *after* the two retained routes go; doing it first trips the classifier drift
  guard and 403s both endpoints for all callers.
- "Re-point the dashboard at the generic `EdgionConfigData` PUT and then delete these" —
  possible in principle, but it would move the base+overlay merge into the dashboard, which
  contradicts the standing rule that the Controller is the single authority for the effective
  view.
- "The endpoints have no Center-side caller, so they are dead" — the caller is the web layer
  going through the proxy tunnel; look in `web/src`, not in `crates/`.

## Re-evaluation Triggers

Re-open this decision only if:

- The Controller-perspective RegionRoute page is removed from the Center dashboard (then the
  whole surface, including the synthetic kind and `Verb::Failover`, becomes deletable in one
  commit with the routes).
- The effective-view merge moves out of the Controller, making `region-routes/effective`
  redundant.
- A second synthetic authorization kind appears — at that point the "one irregular shape"
  justification no longer holds and the model should be reworked rather than extended.

## Reference Cases

- Task: `EdgionCenter/tasks/new_work/06-delete-controller-region-route-surface.md` (retention
  decision recorded 2026-07-27) and its dependency
  `04-delete-region-routes-map.md` (closed), whose narrowed scope produced this outcome.
- Code: `edgion-controller/src/api/region_route_handlers.rs`,
  `edgion-controller/src/api/authz_classifier.rs`,
  `edgion-controller/src/fed_sync/default_policy.rs` (Rule 3).
- Consumer: `EdgionCenter/web/src/pages/RegionRoute/RegionRouteList.tsx`,
  `EdgionCenter/web/src/api/regionRoute.ts`, `EdgionCenter/web/src/api/client.ts` (proxy
  base-URL rewrite).
- Related concurrency decision on the surviving write path:
  [`../concurrency/failover-read-modify-write-per-key-lock.md`](../concurrency/failover-read-modify-write-per-key-lock.md).
