---
name: controller-requeue
description: 跨资源 Requeue 机制：触发路径、环检测、post-init 重校验、handler 清单。
---

# 跨资源 Requeue 机制

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/10-requeue-mechanism.md`

## 待填充内容

### Requeue 模式

<!-- TODO: 当资源 A 变更时，依赖它的资源 B 被重新处理 -->

### 核心组件

<!-- TODO: Workqueue (per-kind), PROCESSOR_REGISTRY (全局), TriggerChain (因果追踪) -->

### 触发路径

<!-- TODO:
1. gateway_route_index: Route ↔ Gateway (hostname/port 变更)
2. SecretRefManager: Secret → dependents (Gateway, EdgionTls, EdgionPlugins 等)
3. ServiceRefManager: Service → routes (所有路由类型)
4. CrossNamespaceRefManager: ReferenceGrant → 跨命名空间资源
5. ListenerPortManager: Gateway → Gateway (端口冲突)
6. AttachedRouteTracker: Route → 父 Gateway (用于 status)
-->

### 环检测

<!-- TODO: 最多 5 次触发循环，DAG 约束（资源类型间无环） -->

### Post-init 重校验

<!-- TODO: 3 个函数在所有 processor ready 后运行（cross-ns, secret, route 校验） -->

### Handler 清单

<!-- TODO: 每种资源类型的 on_change/on_delete 要求 -->
