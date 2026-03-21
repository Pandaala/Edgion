---
name: controller-cache-server
description: ServerCache + EventStore：内存缓存设计、环形事件缓冲区、sync_version 追踪、变更通知。
---

# CacheServer（内存缓存）

> **状态**: 框架已建立，待填充详细内容。

## 待填充内容

### ServerCache<T>

<!-- TODO: 泛型缓存，持有资源及事件历史 -->
<!-- 存储资源到环形事件缓冲区 -->
<!-- 追踪 sync_version（单调递增） -->
<!-- 变更时通知 watcher -->
<!-- 支持 list 快照用于 Gateway 初始同步 -->

### EventStore<T>

<!-- TODO: 环形事件缓冲区 -->
<!-- 存储 Add/Update/Delete 事件 -->
<!-- 有界容量防止内存增长 -->
<!-- 支持事件窗口查询（用于 watch 续接） -->

### 数据流

<!-- TODO:
ResourceProcessor → cache.update() → 通知 watchers → WatchClient
-->

### 与 ConfigSyncServer 的关系

<!-- TODO: Processors 通过 WatchObj 接口注册其 cache -->
