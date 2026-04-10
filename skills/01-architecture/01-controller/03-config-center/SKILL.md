---
name: config-center
description: 配置中心子系统：ConfCenter trait 抽象、FileSystemCenter 和 KubernetesCenter 两种实现、统一的 Workqueue + ResourceProcessor 流水线。
---

# 03 配置中心

> 配置中心是 Controller 的核心子系统，负责从不同来源接收资源配置。
> 两种后端共享相同的 Workqueue + ResourceProcessor 流水线，仅事件源和持久化方式不同。

## ConfCenter trait 抽象

配置中心通过 `ConfCenter` trait 定义统一接口，包含两组能力：

- **CenterApi** — 资源 CRUD 操作：`get_one`、`set_one`、`create_one`、`update_one`、`list`
- **CenterLifeCycle** — 生命周期管理：`start`、`ready check`、`reload`、`shutdown`

无论底层是文件系统还是 Kubernetes API，上层的 Workqueue 和 ResourceProcessor 流水线完全一致。事件源产生的资源变更统一进入 Workqueue 去重和退避，再由 ResourceProcessor 执行校验、预解析、处理和分发。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-overview.md](00-overview.md) | ConfCenter trait 架构 | 理解配置中心抽象设计 |
| [01-file-system.md](01-file-system.md) | FileSystemCenter 实现 | 本地开发/调试模式 |
| [02-kubernetes/](02-kubernetes/SKILL.md) | Kubernetes 配置中心 | 生产环境、HA、Leader 选举 |
| （etcd 无独立文档） | EtcdCenter 实现 | 非 K8s 生产环境，单 Controller 部署 |

## 架构对比

```
ConfCenter trait
├── CenterApi     (CRUD: get_one, set_one, create_one, update_one, list)
└── CenterLifeCycle (start, ready check, reload, shutdown)

FileSystemCenter      KubernetesCenter         EtcdCenter
├── 本地 YAML 文件     ├── K8s API Server        ├── etcd KV 存储
├── inotify 文件监听   ├── Reflector 监听         ├── etcd watch 事件
├── .status 文件       ├── Status Subresource    ├── 无状态持久化
├── 无 Leader 选举     ├── Lease-based 选举       ├── 无 Leader 选举
└── 适用：开发/测试    └── 适用：生产 K8s         └── 适用：非 K8s 生产
```

**EtcdCenter 设计约束**：`EtcdStorage` 的 `create_one`/`update_one` 是非原子的 check-then-write，
设计前提是**单 Controller 独占 etcd**，不支持多 Controller 并发写入同一 etcd 实例。
