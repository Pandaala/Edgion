---
name: controller-architecture
description: edgion-controller control plane architecture — overall design, startup/shutdown, Admin API, FileSystem/Kubernetes/Etcd ConfigCenter backends, ControllerRuntime, Workqueue, ResourceProcessor, Requeue, CacheServer, and ACME service.
---

# 01 Controller Architecture (edgion-controller)

> edgion-controller is the control plane responsible for receiving, validating, processing, and dispatching resources.
> It receives resource configuration from the K8s API, local files, or Etcd, then validates / preparses / processes it and syncs the result to Gateway instances via gRPC.

## File List

| File | Topic | Recommended reading scenario |
|------|-------|------------------------------|
| [00-overview.md](00-overview.md) | Overall Controller architecture | First-time learners of the Controller design |
| [01-startup-shutdown.md](01-startup-shutdown.md) | Process startup and shutdown flow | Debugging startup issues, understanding the initialization order |
| [02-admin-api.md](02-admin-api.md) | Admin HTTP API (:12101) | Using or extending the management interface |
| [03-config-center/](03-config-center/SKILL.md) | ConfigCenter subsystem (ConfCenter trait + three backends) | Modifying resource ingestion, persistence, or K8s HA |
| [04-workqueue.md](04-workqueue.md) | Workqueue dedup and delayed-requeue mechanism | Debugging processing latency, understanding dedup and requeue logic |
| [05-resource-processor.md](05-resource-processor.md) | ResourceProcessor processing pipeline | Modifying resource processing logic, understanding the Handler interface |
| [06-requeue-mechanism.md](06-requeue-mechanism.md) | Cross-resource requeue mechanism | Investigating dependency interactions, understanding TriggerChain |
| [07-cache-server.md](07-cache-server.md) | ServerCache + EventStore in-memory cache | Debugging the gRPC sync data source, understanding the version mechanism |
| [08-acme-service.md](08-acme-service.md) | ACME certificate automation service | Modifying or debugging automatic certificate issuance and renewal |
| [09-add-new-resource/](09-add-new-resource/00-guide.md) | Add-new-resource workflow + 4 template categories | When you need to add a new ResourceKind |
| [10-status-management.md](10-status-management.md) | update_status implementation rules and pitfalls | Implementing or modifying CRD status writeback |
| [11-authentication-authorization.md](11-authentication-authorization.md) | Authentication + authorization architecture (token-only admin authn / CLI token / federation mTLS, authz engine, middleware onion, default federation policy) | Understanding or extending Controller auth/authz, adding CLI tokens, debugging 401/403/503 |

## Architecture Overview

```
                         edgion-controller
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  ConfCenter (FileSystem / Kubernetes / Etcd)                │
│  ├── ResourceController (per-kind watcher)                  │
│  │   └── Workqueue (dedup + delayed requeue)               │
│  │       └── ResourceProcessor                              │
│  │           ├── validate → preparse → parse → on_change    │
│  │           └── update ServerCache                         │
│  │                                                          │
│  ├── ControllerRuntime (per-ConfCenter state owner)         │
│  │   ├── ProcessorRegistry / SecretStore                    │
│  │   └── Reference managers and route/index stores          │
│  └── RequeueChain (cross-resource interactions)             │
│                                                             │
│  Admin API (:12101)                                         │
│  ├── CRUD endpoints (namespaced / cluster)                  │
│  └── reload / diagnostics / server-info                     │
│  Probe (:12100 health/ready) + Metrics (:12190)             │
│                                                             │
│  ConfigSyncServer (gRPC :12151)                             │
│  ├── List / Watch (per-kind)                                │
│  ├── WatchServerMeta                                        │
│  └── ClientRegistry (connected Gateways)                    │
│                                                             │
│  ACME Service (optional, Leader only)                       │
│  └── Let's Encrypt automatic certificate issuance/renewal   │
└─────────────────────────────────────────────────────────────┘
```
