# 配置中心（Config Center）

> Controller 的核心子系统，负责从不同配置源加载资源、处理变更、同步到 Gateway。
> 本目录包含配置中心各层次的详细文档。

## 文件清单

| 文件 / 目录 | 主题 | 推荐阅读场景 |
|-------------|------|-------------|
| [00-overview.md](00-overview.md) | 配置中心通用架构 | 理解 ConfCenter 抽象、Workqueue、ResourceProcessor |
| [01-file-system.md](01-file-system.md) | FileSystemCenter | 本地 YAML 开发/测试模式 |
| [02-kubernetes/](02-kubernetes/) | KubernetesCenter（详细） | K8s 部署相关的全部内容 |

## 架构总览

```
ConfMgr (facade, manager.rs)
└── Arc<dyn ConfCenter>
    ├── FileSystemCenter   — watches local YAML directory
    └── KubernetesCenter   — K8s API watchers + leader election + HA
```

Controller 通过 `ConfCenter` trait 抽象配置源，支持两种后端：

- **FileSystemCenter**：监听本地 YAML 目录，用于开发/测试
- **KubernetesCenter**：通过 K8s API watchers 监听 CRD 变更，支持多副本 HA

两种后端共享完全相同的 Workqueue + ResourceProcessor 处理管线，
差异仅在于事件来源和资源持久化方式。
