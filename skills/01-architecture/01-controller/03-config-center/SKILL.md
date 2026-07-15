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
| (etcd has no standalone document) | EtcdCenter implementation | Non-K8s production; supports active-passive HA (multiple Controllers) via leader election |

## Architecture Comparison

```
ConfCenter trait
├── CenterApi     (CRUD: get_one, set_one, create_one, update_one, list)
└── CenterLifeCycle (start, ready check, reload, shutdown)

FileSystemCenter      KubernetesCenter         EtcdCenter
├── Local YAML files  ├── K8s API Server        ├── etcd KV store
├── inotify watching  ├── Reflector watch        ├── etcd watch events
├── .status files     ├── Status subresource    ├── No status persistence
├── No leader elect   ├── Lease-based election  ├── v3election + lease keepalive
└── Use: dev/test     └── Use: production K8s   └── Use: non-K8s production (HA)
```

**EtcdCenter HA**: `EtcdStorage::create_one`/`update_one` are non-atomic check-then-write operations, so a single active writer is still required. Multiple Controllers may now run **active-passive**: etcd `v3election` leader election plus a self-managed lease keepalive ensures exactly one active leader (which runs the resource controllers and serves conf_sync), while standbys stand by and take over on failover. A pre-write leadership gate backs up the election on the write path. See `docs/superpowers/specs/2026-07-14-etcd-leader-election-design.md` for the full design.
