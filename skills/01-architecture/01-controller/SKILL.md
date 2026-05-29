---
name: controller-architecture
description: edgion-controller control plane architecture — overall design, startup/shutdown, Admin API, ConfigCenter, Workqueue, ResourceProcessor, Requeue, CacheServer, ACME service.
---

# 01 Controller Architecture (edgion-controller)

> edgion-controller is the control plane responsible for receiving, validating, processing, and dispatching resources.
> It receives resource configuration from K8s API or the local file system, then validates / preparses / processes it and syncs the result to Gateway instances via gRPC.

## File List

| File | Topic | Recommended reading scenario |
|------|-------|------------------------------|
| [00-overview.md](00-overview.md) | Overall Controller architecture | First-time learners of the Controller design |
| [01-startup-shutdown.md](01-startup-shutdown.md) | Process startup and shutdown flow | Debugging startup issues, understanding the initialization order |
| [02-admin-api.md](02-admin-api.md) | Admin HTTP API (:5800) | Using or extending the management interface |
| [03-config-center/](03-config-center/SKILL.md) | ConfigCenter subsystem (ConfCenter trait + dual backends) | Modifying the resource processing flow, K8s HA |
| [04-workqueue.md](04-workqueue.md) | Workqueue dedup and delayed-requeue mechanism | Debugging processing latency, understanding dedup and requeue logic |
| [05-resource-processor.md](05-resource-processor.md) | ResourceProcessor processing pipeline | Modifying resource processing logic, understanding the Handler interface |
| [06-requeue-mechanism.md](06-requeue-mechanism.md) | Cross-resource requeue mechanism | Investigating dependency interactions, understanding TriggerChain |
| [07-cache-server.md](07-cache-server.md) | ServerCache + EventStore in-memory cache | Debugging the gRPC sync data source, understanding the version mechanism |
| [08-acme-service.md](08-acme-service.md) | ACME certificate automation service | Modifying or debugging automatic certificate issuance and renewal |
| [09-add-new-resource/](09-add-new-resource/00-guide.md) | Add-new-resource workflow + 4 template categories | When you need to add a new ResourceKind |
| [10-status-management.md](10-status-management.md) | update_status implementation rules and pitfalls | Implementing or modifying CRD status writeback |
| [11-authentication-authorization.md](11-authentication-authorization.md) | Authentication + authorization architecture (unified_auth / cli token / fed mTLS, the authz engine, the middleware onion, default Center policy) | Understanding or extending Controller auth/authz, adding cli tokens, debugging 401/403 |

## Architecture Overview

```
                         edgion-controller
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│  ConfCenter (FileSystem / Kubernetes)                       │
│  ├── ResourceController (per-kind watcher)                  │
│  │   └── Workqueue (dedup + delayed requeue)               │
│  │       └── ResourceProcessor                              │
│  │           ├── validate → preparse → parse → on_change    │
│  │           └── update ServerCache                         │
│  │                                                          │
│  ├── ProcessorRegistry (global registry)                    │
│  ├── SecretRefManager / ServiceRefManager                   │
│  ├── CrossNamespaceRefManager                               │
│  └── RequeueChain (cross-resource interactions)             │
│                                                             │
│  Admin API (:5800)                                          │
│  ├── CRUD endpoints (namespaced / cluster)                  │
│  ├── health / ready                                         │
│  └── reload / server-info                                   │
│                                                             │
│  ConfigSyncServer (gRPC :50051)                             │
│  ├── List / Watch (per-kind)                                │
│  ├── WatchServerMeta                                        │
│  └── ClientRegistry (connected Gateways)                    │
│                                                             │
│  ACME Service (optional, Leader only)                       │
│  └── Let's Encrypt automatic certificate issuance/renewal   │
└─────────────────────────────────────────────────────────────┘
```
