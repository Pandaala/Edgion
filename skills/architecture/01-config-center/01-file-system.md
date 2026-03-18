# FileSystemCenter

> 基于本地文件系统的配置中心，用于开发和测试场景。

## 概述

`FileSystemCenter` 监听本地 YAML 目录，将文件变更转化为资源事件，
经过与 KubernetesCenter 完全相同的 Workqueue + ResourceProcessor 管线处理。

## 工作原理

```
YAML 目录
├── Resource/
│   ├── Item/
│   │   ├── namespace/
│   │   │   └── name.yaml        # 资源定义
│   │   │   └── name.status      # Status 持久化
```

- **启动时：** 扫描目录，对每个 YAML 文件执行 `on_init_apply()`
- **运行时：** 使用 `notify` 库监听文件系统事件（create/modify/delete）
- **Status 持久化：** 写入 `.status` 文件（而非 K8s API）

## 与 KubernetesCenter 的差异

| 方面 | FileSystemCenter | KubernetesCenter |
|------|-----------------|-----------------|
| 事件来源 | 文件系统 inotify/kqueue | K8s API watch stream |
| Status 持久化 | `.status` 文件 | K8s API `patch /status` |
| Leader Election | 无（单实例） | Lease-based |
| 适用场景 | 开发、测试、CI | 生产 K8s 部署 |

## 关键文件

- `src/core/controller/conf_mgr/conf_center/file_system/center.rs` — `FileSystemCenter`
- `src/core/controller/conf_mgr/conf_center/file_system/storage.rs` — 文件系统存储
- `src/core/controller/conf_mgr/conf_center/file_system/watcher.rs` — 文件系统事件监听
