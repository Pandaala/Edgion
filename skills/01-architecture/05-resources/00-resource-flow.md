---
name: resource-flow
description: 资源通用处理流程：从 K8s/文件系统变更到 Controller 处理再到 Gateway 运行时生效的完整链路。
---

# 资源通用处理流程

> **状态**: 框架已建立，待填充详细内容。

## 概要

每种资源都遵循相同的基础流程从来源到最终生效。本文描述这个通用流程，各资源的特殊处理见各自文档。

## 待填充内容

### 阶段一：资源来源

<!-- TODO:
- Kubernetes 模式：K8s API Server → Reflector 监听 → Event
- FileSystem 模式：本地 YAML → inotify 监听 → Event
-->

### 阶段二：Controller 处理

<!-- TODO:
1. 入队 Workqueue（去重）
2. Worker 出队
3. namespace 过滤
4. validate() — 校验
5. preparse() — 预解析，构建运行时结构
6. parse() — 解析，解析引用
7. 提取 status
8. 更新 status（检测变更）
9. check change — 检查是否有实质变更
10. on_change() — 处理变更（更新依赖、触发 requeue）
11. 更新 ServerCache
-->

### 阶段三：gRPC 同步

<!-- TODO:
- ServerCache 变更通知
- ConfigSyncServer 推送 WatchResponse 给 Gateway
- 包含 sync_version 用于续接
-->

### 阶段四：Gateway 接收

<!-- TODO:
- ConfigSyncClient 接收事件
- ClientCache 更新本地缓存
- EventDispatch 分发给对应 ConfHandler
-->

### 阶段五：Gateway 生效

<!-- TODO:
- ConfHandler 处理变更
- 更新运行时状态（路由表、TLS 证书、LB 配置等）
- 通过 ArcSwap 原子切换（对请求处理无锁）
-->

### 跨资源联动

<!-- TODO:
- 资源 A 变更可能触发资源 B 的 requeue
- 通过 SecretRefManager、ServiceRefManager 等追踪依赖
- TriggerChain 防止循环
- 参见 01-controller/06-requeue-mechanism.md
-->

### Status 回写

<!-- TODO:
- Controller 在处理完成后更新 Status
- Kubernetes 模式：写入 status subresource
- FileSystem 模式：写入 .status 文件
- 仅 Leader 执行回写
-->
