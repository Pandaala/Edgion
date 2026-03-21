---
name: controller-startup-shutdown
description: edgion-controller 启动与关闭流程：初始化顺序、Leader 选举、事件驱动主流程、优雅关闭。
---

# Controller 启动与关闭

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 启动流程

<!-- TODO: EdgionControllerCli::run() 完整启动序列 -->
<!-- 1. 解析命令行参数
     2. 初始化工作目录和日志
     3. 加载 CRD Schema（FileSystem 模式）
     4. 创建 ConfMgr（根据配置选择 FileSystem 或 Kubernetes）
     5. 启动 ConfMgr（含 shutdown signal 处理）
     6. 启动 Admin API HTTP 服务
     7. 启动 gRPC ConfigSyncServer
     8. 等待关闭信号 -->

### Leader 选举（Kubernetes 模式）

<!-- TODO:
- Lease-based 选举机制
- leader-only vs all-serve 两种 HA 模式
- LeaderHandle: Arc<AtomicBool>
- Pod label 更新 (edgion.io/leader)
- 事件驱动主流程：CachesReady, LeadershipLost, LeadershipAcquired 等
-->

### HA 模式

<!-- TODO:
- leader-only: Leader 运行 watchers/gRPC/status/ACME；follower 待命
- all-serve: 所有副本运行 watchers/gRPC；仅 Leader 运行 status/ACME
- 行为矩阵：Watcher 启动时机、gRPC 可用性、status 回写、ACME、K8s 负载
- 失败转移时间
-->

### 事件驱动主循环

<!-- TODO: Watcher 任务 — Controller, Caches, Leader, Reload -->

### ConfigSyncServer 启动时机

<!-- TODO: 必须等待所有 phased processors 注册 + 所有 sync kinds ready -->

### 优雅关闭

<!-- TODO: ShutdownHandle 传播、Controller 完成当前处理后退出 -->
