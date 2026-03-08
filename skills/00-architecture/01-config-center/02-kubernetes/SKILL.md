# Kubernetes 配置中心

> KubernetesCenter 的完整架构文档，涵盖生命周期管理、Leader Election、HA 多副本模式、
> ResourceController 工作机制。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-lifecycle.md](00-lifecycle.md) | 生命周期与 Leader Election | 理解 Controller 启动流程、Leader 选举 |
| [01-ha-mode.md](01-ha-mode.md) | HA 模式（leader-only / all-serve） | 配置多副本高可用、理解两种模式差异 |
| [02-resource-controller.md](02-resource-controller.md) | ResourceController 与 Status 写入 | 理解资源处理流程、status 回写机制 |

## 架构总览

```
KubernetesCenter (implements ConfCenter = CenterApi + CenterLifeCycle)
│
├── writer: KubernetesStorage (CenterApi delegate → K8s API CRUD)
│
├── config: KubernetesConfig
│   ├── gateway_class, controller_name
│   ├── watch_namespaces, label_selector
│   ├── leader_election: LeaderElectionConfig
│   ├── metadata_filter: MetadataFilterConfig
│   └── ha_mode: HaMode (leader-only | all-serve)
│
└── lifecycle (CenterLifeCycle impl)
    │
    ├── LeaderElection (Lease-based)
    │   ├── Pod label: edgion.io/leader
    │   └── LeaderHandle (shared AtomicBool)
    │
    └── KubernetesController
        │
        ├── spawn::<HTTPRoute, _>(HttpRouteHandler)
        │   └── ResourceProcessor + ResourceController
        ├── spawn::<Gateway, _>(GatewayHandler)
        │   └── ResourceProcessor + ResourceController
        └── ... (~20 resource types)
```

## 关键文件

| 文件 | 职责 |
|------|------|
| `conf_center/kubernetes/config.rs` | `KubernetesConfig`, `HaMode`, `LeaderElectionConfig` |
| `conf_center/kubernetes/center.rs` | `KubernetesCenter` 生命周期 |
| `conf_center/kubernetes/leader_election.rs` | `LeaderElection`, `LeaderHandle` |
| `conf_center/kubernetes/controller.rs` | `KubernetesController`（spawn 所有 ResourceController） |
| `conf_center/kubernetes/resource_controller.rs` | `ResourceController<K>`（单资源生命周期） |
| `conf_center/kubernetes/storage.rs` | `KubernetesStorage`（K8s API CRUD） |
| `conf_mgr/processor_registry.rs` | `PROCESSOR_REGISTRY`（全局处理器注册表） |
