---
name: controller-workqueue
description: Workqueue 机制：Go controller-runtime 风格、去重、指数退避、延迟入队、TriggerChain 环检测。
---

# Workqueue 机制

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### 设计理念

<!-- TODO: Go controller-runtime 风格的工作队列 -->

### 去重机制

<!-- TODO: 同一 key 在队列中只存在一次 -->

### 指数退避

<!-- TODO: 自动重试，延迟递增至 max_backoff -->

### 延迟入队

<!-- TODO: 跨资源 requeue 时的合并入队 -->

### Dirty requeue

<!-- TODO: Key 在 dequeue 时从 pending 移除，允许处理期间重新入队 -->

### WorkItem 结构

<!-- TODO:
```rust
pub struct WorkItem {
    pub key: String,           // "namespace/name" 格式
    pub retry_count: u32,      // 重试次数
}
```
-->

### TriggerChain

<!-- TODO: 级联路径追踪，防止无限 requeue 循环 -->
<!-- 示例：HTTPRoute/def → Gateway/def→gw1 → HTTPRoute/def，超过 2 次则停止 -->

### WorkqueueConfig

<!-- TODO: 配置项：max_retries, base_delay, max_backoff 等 -->
