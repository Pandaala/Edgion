---
name: config-center
description: ConfigCenter subsystem — ConfCenter trait abstraction, FileSystemCenter/KubernetesCenter/EtcdCenter implementations, and the shared ControllerRuntime + Workqueue + ResourceProcessor pipeline.
---

# 03 ConfigCenter

> The ConfigCenter is a core Controller subsystem responsible for receiving resource configuration from various sources.
> All three backends share the same ControllerRuntime, Workqueue, and ResourceProcessor model; event sources, persistence, HA, and service ownership differ.

## ConfCenter Trait Abstraction

The ConfigCenter defines a unified interface via the `ConfCenter` trait, which contains two groups of capabilities:

- **CenterApi** — resource CRUD operations: `get_one`, `set_one`, `create_one`, `update_one`, `list`
- **CenterLifeCycle** — lifecycle management: `start`, `ready check`, `reload`, `shutdown`

Regardless of whether the source is the file system, Kubernetes API, or Etcd, resource changes enter the same runtime-owned Workqueue and ResourceProcessor pipeline for deduplication, validation, preparse, processing, status, and dispatch.

## File List

| File | Topic | Recommended reading scenario |
|------|-------|------------------------------|
| [00-overview.md](00-overview.md) | ConfCenter trait architecture | Understanding the ConfigCenter abstraction design |
| [01-file-system.md](01-file-system.md) | FileSystemCenter implementation | Local development / debugging mode |
| [02-kubernetes/](02-kubernetes/SKILL.md) | Kubernetes ConfigCenter | Production environments, HA, leader election |
| [00-overview.md](00-overview.md#backend-comparison) | EtcdCenter summary and source entry points | Etcd persistence and watch behavior |

## Architecture Comparison

```
ConfCenter trait
├── CenterApi     (CRUD: get_one, set_one, create_one, update_one, list)
└── CenterLifeCycle (start, ready check, reload, shutdown)

FileSystemCenter      KubernetesCenter         EtcdCenter
├── Local YAML files  ├── K8s API Server        ├── etcd KV store
├── inotify watching  ├── Reflector watch        ├── etcd watch events
├── .status files     ├── Status subresource    ├── Status key prefix
├── No leader elect   ├── Lease-based election  ├── No leader election
└── Use: dev/test     └── Use: production K8s   └── Use: external KV storage
```

Etcd resource create/update operations use conditional transactions, status is
stored under the status prefix, and watchers track Etcd revisions. Consult
`edgion-controller/src/conf_mgr/conf_center/etcd/` before making concurrency or
multi-controller guarantees; do not infer them from the old check-then-write implementation.
