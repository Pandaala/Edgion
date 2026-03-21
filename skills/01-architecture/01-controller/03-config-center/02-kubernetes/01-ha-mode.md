---
name: kubernetes-ha-mode
description: Kubernetes 高可用模式：leader-only vs all-serve 行为矩阵、失败转移、status 回写守卫、ACME leader-only。
---

# Kubernetes HA 模式

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/01-config-center/02-kubernetes/01-ha-mode.md`

## 待填充内容

### leader-only 模式

<!-- TODO: Leader 运行 watchers/gRPC/status/ACME；follower 待命 -->

### all-serve 模式

<!-- TODO: 所有副本运行 watchers/gRPC；仅 Leader 运行 status/ACME -->

### 行为矩阵

<!-- TODO: Watcher 启动时机、gRPC 可用性、status 回写、ACME、leadership loss 处理、K8s 负载、内存 -->

### Status 回写守卫

<!-- TODO: persist_k8s_status() 由 leader_handle 守卫 -->

### Leader 切换

<!-- TODO: 新 Leader 调用 PROCESSOR_REGISTRY.requeue_all() 做全量 status 对账 -->

### server_id 与 Gateway 重连

<!-- TODO: 每副本不同 server_id；Gateway 在 server_id 变化时全量 relist -->
