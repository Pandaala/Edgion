---
name: config-center
description: 配置中心子系统：ConfCenter trait 抽象、FileSystemCenter 和 KubernetesCenter 两种实现、统一的 Workqueue + ResourceProcessor 流水线。
---

# 03 配置中心

> 配置中心是 Controller 的核心子系统，负责从不同来源接收资源配置。
> 两种后端共享相同的 Workqueue + ResourceProcessor 流水线，仅事件源和持久化方式不同。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-overview.md](00-overview.md) | ConfCenter trait 架构 | 理解配置中心抽象设计 |
| [01-file-system.md](01-file-system.md) | FileSystemCenter 实现 | 本地开发/调试模式 |
| [02-kubernetes/](02-kubernetes/SKILL.md) | Kubernetes 配置中心 | 生产环境、HA、Leader 选举 |

## 架构对比

```
ConfCenter trait
├── CenterApi     (CRUD: get_one, set_one, create_one, update_one, list)
└── CenterLifeCycle (start, ready check, reload, shutdown)

FileSystemCenter                    KubernetesCenter
├── 本地 YAML 文件                   ├── K8s API Server
├── inotify 文件监听                 ├── Reflector 监听
├── .status 文件持久化               ├── Status Subresource 回写
├── 无 Leader 选举                   ├── Lease-based Leader 选举
└── 适用：开发/测试                  └── 适用：生产环境
```
