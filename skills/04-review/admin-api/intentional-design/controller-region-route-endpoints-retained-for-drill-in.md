---
name: controller-region-route-endpoints-retained-for-drill-in
description: Controller-owned RegionRoute observations and preview-bound single-setting operations remain distinct from generic source CRUD.
status: accepted-tradeoff
finding_id: new_work-06
closed: 2026-07-27
knowledge_type: intentional-design
---

# Controller-owned regional observations and source commands

## Current contract

The ClusterRegionSetting migration supersedes the old overlay/base-table model
and the old unversioned failover route. No legacy route alias remains.
`GET /api/v1/region-routes/effective` still returns a limited Controller observation
DTO. Source directory keys/detail use EdgionConfigData list/get permission;
consumer observations use synthetic RegionRoute list permission; setting-scoped
preview/apply use Verb::Failover on RegionRoute.

The dedicated command validates the complete proposed table and requires source
content equality before idempotence or mutation. Generic PUT is not a substitute
for its preview, validation and persistence-outcome contract. Consumer compatibility
uses the proposed/source table and Controller-observed reference authorization;
it does not claim Gateway application. Existing unavailable consumers are reported
rather than creating a new all-consumers-healthy gate.

Apply is mounted behind the existing write-readiness gate. Fresh non-Kubernetes
resourceVersion allocation also skips a value equal to the current source version,
including a version supplied by an external editor after startup. Source equality
includes UID, version and unknown fields; Kubernetes uses native versioned replace,
etcd compares modification revision, and filesystem mode uses its existing process
lock through actual blocking filesystem completion. External file edits are not
atomically fenced. Timeout is an unknown write outcome, not evidence of no write.

Center migration is tracked with the source-browsing task; do not infer a working
cross-repository caller from this Controller-only contract or historical UI paths.
See [the implementation task](../../../../tasks/todo/center-source-resource-browsing/02-implementation.md).

## Historical evidence and re-evaluation

The 2026-07-27 finding correctly established that dedicated effective computation
and validated single-target commands were not dead code merely because fleet-wide
operations had generic PUT callers. Its old UI paths, overlay merge description,
and endpoint names are historical and cannot authorize their preservation.
Recheck actual Center consumers, router classification and tests whenever this
surface changes. The synthetic permission remains necessary while these routes
exist; no wildcard permission may bypass the separate federation source boundary.
