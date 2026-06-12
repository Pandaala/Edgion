---
name: gateway-no-local-config-snapshot
description: Use when reviewing findings that compare Edgion against Envoy/Istio and claim "Gateway keeps config in memory only / cold restart blackholes traffic until full resync / no last-good disk snapshot / no warm-start / Controller restart forces full relist". Covers why disk-backed warm-start is a large cross-layer feature (not a narrow review fix) AND why the gap is not actually below the Envoy/Istio default baseline.
status: accepted-tradeoff
finding_id: sync-05
closed: 2026-05-31
---

# No local config snapshot / warm-start on Gateway cold restart — ACCEPTED-TRADEOFF (matches Envoy/Istio default baseline)

## Conclusion

The Gateway holds all synced configuration in memory only. A Gateway **process**
cold start (pod recreate, OOMKill, node drain, rolling image upgrade) begins with
empty `ClientCache`s (`CacheData::new()` → `ready: false`, empty `data`,
`edgion-gateway/src/conf_sync/cache_client/cache_data.rs`); `wait_for_ready`
(`edgion-gateway/src/cli/mod.rs`) polls until every kind has completed its initial
List before Pingora starts accepting connections, so a cold Gateway serves no
traffic until the first full sync completes. The TODO at
`edgion-gateway/src/conf_sync/conf_client/grpc_client.rs:27-32` already acknowledges
this gap.

This is an accepted design tradeoff, **not** a defect and **not** a narrow review
fix. The proposed "persist last-good config to disk + warm-load + tentatively-ready
staleness flag + incremental Watch via server_id/sync_version" is a large
cross-layer feature (snapshot store + startup load path + cache readiness semantics
+ conf-sync protocol change) with medium risk (serving stale config to live
traffic, cross-kind consistency). If ever prioritized it must go through
brainstorming as a `tasks/todo/` feature, not this workflow.

## Core Rationale

**1. Edgion already has the resilience property that matters; the gap is narrower than the finding implies**

The valuable xDS resilience property — "control plane down does NOT take the data
plane down" — Edgion already satisfies: config is in memory, so a running Gateway
whose Controller dies keeps serving with last-good config and re-syncs on
reconnect (lazy reconnect, `grpc_client.rs`). The finding's title ("after Controller
restart until full resync") does not hold for an **already-running** Gateway. The
real gap is only the Gateway **process cold start** path.

**2. The cold-start gap is NOT below the Envoy/Istio default baseline**

The finding's premise mischaracterizes Envoy. Envoy keeps last-good config in
**memory** too — and loses it on process restart exactly like Edgion. The cited
`--restart-epoch` hot restart is **not** disk persistence: it is a same-host
handoff between two **live** processes over shared memory + a Unix domain socket
(inherits sockets/stats, graceful drain), and it requires the old process to still
be alive. A K8s pod recreate / node move (no old process to hand off from) gets
zero benefit from it, so a fresh Envoy/Istio sidecar also starts with empty dynamic
xDS and must reach istiod before it can route. "New pods cannot get config while
istiod is down" is a well-known Istio limitation. Disk-backed warm-load is not a
stock Envoy default — it only exists as a deployment technique via filesystem
(path-based) xDS, i.e. an external component writing config to disk. So Edgion is
**at parity** with the Envoy/Istio default here, not behind it.

**3. The genuine exposure is rare and largely covered by deployment hygiene**

A single Gateway pod restart is handled correctly by the readiness probe (the cold
pod is excluded from Service endpoints) plus multiple replicas + PodDisruptionBudget
— peers serve while the new pod syncs. The only scenario the snapshot feature would
materially improve is a **correlated** restart of the Controller *and all* Gateways
at once (whole node-pool / cluster failure), which is uncommon and where the
recovery is bounded by Controller re-sync from the K8s API server anyway.

## Fix Suggestions Not Accepted

- "Persist last-good config to local disk and warm-load it on cold start" — large
  cross-layer feature (snapshot store + load path + Pingora startup change), beyond
  the narrow review-fix scope. Promote to `tasks/todo/` via brainstorming if ever
  prioritized.
- "Mark caches tentatively-ready with a staleness flag and start Pingora before sync
  completes" — changes readiness semantics and risks serving stale config to live
  traffic; also reintroduces the cross-kind consistency hazard
  (`sync-02-no-cross-kind-ordering.md`) at startup.
- "Add server_id/sync_version to enable an incremental Watch after reconnect on cold
  start" — only useful once persisted state exists; it is part of the disk-snapshot
  feature, not an independently shippable fix.
- "Treat the cold-start traffic gap as a regression vs Envoy" — rejected on the
  facts: stock Envoy/Istio have the same cold-restart gap (see Rationale 2).

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows a correlated Controller + full-Gateway-fleet restart
  causing a measured outage that multi-replica + PDB cannot cover.
- A supported deployment topology requires single-Gateway-replica restart resilience.
- The project adds disk-backed config for another reason, making warm-start an
  incremental rather than greenfield change.

## Reference Cases

- `tasks/review-envoy/sync-05-no-local-snapshot-persistence.md` (closed 2026-05-31, ⚠️ accepted-tradeoff)
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs` (TODO lines 27-32 — the acknowledged gap)
- `edgion-gateway/src/conf_sync/cache_client/cache_data.rs` (`CacheData::new()` → `ready: false`)
- `edgion-gateway/src/cli/mod.rs` (`wait_for_ready` poll loop)
- `skills/04-review/architecture/listener-port-set-frozen-at-startup.md` (sibling Envoy-comparison limitation)
- `tasks/review-envoy/sync-02-no-cross-kind-ordering.md` (cross-kind consistency, referenced by the finding's point 3)
