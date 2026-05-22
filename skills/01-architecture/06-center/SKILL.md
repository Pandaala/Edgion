---
name: center
description: Use when understanding how the edgion-center binary aggregates Controllers, exposes federation gRPC, persists registry state, and surfaces unified API for multi-cluster topologies.
---

# 06 Center Architecture (edgion-center)

Center is the federation hub for multi-cluster Edgion deployments.
It accepts bidirectional gRPC streams from one or more Controllers, aggregates the
PluginMetaData they publish, and exposes an HTTP Admin API for operators.
Entry point: `src/bin/edgion_center.rs`; startup logic: `src/core/center/cli/mod.rs`.

## Mental model

```
Controller A ──┐
Controller B ──┼── gRPC FederationSync (:50052) ──► Center ──► HTTP Admin API (:5810)
Controller C ──┘     (bidirectional stream)           │
                                                       └── SQLite (controller registry)
```

Center is the *server* side of `FederationSync`; each Controller is a *client* that
dials in and sends a `RegisterRequest`, then receives `FedWatchRequest` messages
(Center → Controller direction) and responds with watch data.
This is the *reverse* of the typical K8s informer pattern: Center issues the watch,
Controllers are the data sources.

## File list

| File | Topic |
|------|-------|
| [00-overview.md](00-overview.md) | Deployment shape, module map, lifecycle, failure modes |
| [01-fed-sync-server.md](01-fed-sync-server.md) | `FederationGrpcServer`, `RegisterRequest` validation caps, bidirectional stream protocol |
| [02-aggregator-and-watch-cache.md](02-aggregator-and-watch-cache.md) | `ResourceAggregator`, `CenterWatchCache`, `CenterMetaDataStore`, reverse-Watch data flow |
| [03-persistence.md](03-persistence.md) | SQLite `controllers` table, `CenterDb`, configuration, Admin API |

## See also

- [`04-review/CLOSED-FINDINGS.md`](../../04-review/CLOSED-FINDINGS.md) — `common-center-03` row: validation caps confirmed fixed
- [`04-review/SKILL.md`](../../04-review/SKILL.md) — security review routing table (federation / CPU-memory topics)
