---
name: config-center
description: ConfigCenter subsystem — ConfCenter trait abstraction, FileSystemCenter and KubernetesCenter implementations, unified Workqueue + ResourceProcessor pipeline.
---

# 03 ConfigCenter

> The ConfigCenter is a core Controller subsystem responsible for receiving resource configuration from various sources.
> Both backends share the same Workqueue + ResourceProcessor pipeline; only the event source and persistence layer differ.

## ConfCenter Trait Abstraction

The ConfigCenter defines a unified interface via the `ConfCenter` trait, which contains two groups of capabilities:

- **CenterApi** — resource CRUD operations: `get_one`, `set_one`, `create_one`, `update_one`, `list`
- **CenterLifeCycle** — lifecycle management: `start`, `ready check`, `reload`, `shutdown`

Whether the underlying backend is the file system or the Kubernetes API, the upper-layer Workqueue and ResourceProcessor pipeline is identical. Resource changes from the event source enter the Workqueue uniformly for dedup and delayed (coalesced) requeue, then the ResourceProcessor performs validation, preparse, processing, and dispatch.

## File List

| File | Topic | Recommended reading scenario |
|------|-------|------------------------------|
| [00-overview.md](00-overview.md) | ConfCenter trait architecture | Understanding the ConfigCenter abstraction design |
| [01-file-system.md](01-file-system.md) | FileSystemCenter implementation | Local development / debugging mode |
| [02-kubernetes/](02-kubernetes/SKILL.md) | Kubernetes ConfigCenter | Production environments, HA, leader election |
| (etcd has no standalone document) | EtcdCenter implementation | Non-K8s production, single-Controller deployment |

## Architecture Comparison

```
ConfCenter trait
├── CenterApi     (CRUD: get_one, set_one, create_one, update_one, list)
└── CenterLifeCycle (start, ready check, reload, shutdown)

FileSystemCenter      KubernetesCenter         EtcdCenter
├── Local YAML files  ├── K8s API Server        ├── etcd KV store
├── inotify watching  ├── Reflector watch        ├── etcd watch events
├── .status files     ├── Status subresource    ├── No status persistence
├── No leader elect   ├── Lease-based election  ├── No leader election
└── Use: dev/test     └── Use: production K8s   └── Use: non-K8s production
```

**EtcdCenter design constraint**: `EtcdStorage::create_one`/`update_one` are non-atomic check-then-write operations. The design assumes **a single Controller exclusively owns etcd**; concurrent writes from multiple Controllers to the same etcd instance are not supported.
