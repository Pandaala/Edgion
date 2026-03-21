---
name: kubernetes-lifecycle
description: KubernetesCenter 启动流程与 Leader 选举：Lease-based 选举、事件驱动主循环、Watcher 任务管理。
---

# Kubernetes 配置中心 — 启动与选举

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/01-config-center/02-kubernetes/00-lifecycle.md`

## 待填充内容

### 启动流程

<!-- TODO: Create K8s client → Init LeaderElection → Preflight checks → spawn le.run() → match ha_mode -->

### Leader-only 生命周期

<!-- TODO: wait_until_leader() → run_main_flow() → on exit: clear + loop -->

### All-serve 生命周期

<!-- TODO: run_serving_flow() immediately (no wait) -->

### Leader 选举机制

<!-- TODO: Lease-based, LeaderHandle: Arc<AtomicBool>, Pod label 更新 -->

### 事件驱动主流程

<!-- TODO: CachesReady, CachesTimeout, LeadershipLost, LeadershipAcquired, ControllerExit, ReloadRequested -->

### Watcher 任务

<!-- TODO: Controller, Caches (all phased processors + sync kinds ready), Leader (100ms poll), Reload -->

### ConfigSyncServer 启动时机

<!-- TODO: 必须等待所有 phased processors 注册 + 所有 sync kinds ready -->
