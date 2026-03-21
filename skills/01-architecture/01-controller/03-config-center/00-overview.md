---
name: config-center-overview
description: ConfCenter trait 架构：CenterApi、CenterLifeCycle、Workqueue 流水线、BidirectionalRefManager、Secret 管理。
---

# 配置中心架构总览

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/01-config-center/00-overview.md`

## 待填充内容

### ConfCenter trait

<!-- TODO: CenterApi (CRUD) + CenterLifeCycle (startup/readiness) -->

### Workqueue 流水线

<!-- TODO: Event → ResourceController.on_apply/on_delete → enqueue → Worker loop → validate → preparse → parse → on_change → update_status -->

### Requeue with backoff

<!-- TODO: 指数退避 up to max_backoff, max_retries -->

### Dirty requeue

<!-- TODO: Key 在 dequeue 时释放，允许处理期间重新入队 -->

### BidirectionalRefManager

<!-- TODO: 通用引用追踪（Secret→dependents, Service→routes, ReferenceGrant→cross-ns） -->

### Secret 管理

<!-- TODO: GLOBAL_SECRET_STORE with lazy-lock, 级联 requeue -->
