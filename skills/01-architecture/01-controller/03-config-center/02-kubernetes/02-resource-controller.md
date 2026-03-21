---
name: kubernetes-resource-controller
description: Kubernetes ResourceController 生命周期：Reflector 监听、Init/Runtime 阶段、Status 回写、410 Gone 处理。
---

# ResourceController 生命周期

> **状态**: 框架已建立，待填充详细内容。
> **原文件**: `_01-architecture-old/01-config-center/02-kubernetes/02-resource-controller.md`

## 待填充内容

### Per-resource 架构

<!-- TODO: 每种资源一个 ResourceController，独立并行执行 -->

### Init 阶段 (Steps 1-6)

<!-- TODO: Create reflector → Event::Init → Event::InitApply(obj) with status persist → Event::InitDone → spawn_worker -->

### Runtime 阶段 (Steps 7-8)

<!-- TODO: Event::Apply/Delete → enqueue → worker dequeue/process → status persist if leader -->

### ResourceProcessor 流水线

<!-- TODO: 11 步：namespace filter → validate → preparse → parse → extract status → update status → check change → on_change → ServerCache -->

### Status 回写

<!-- TODO: DynamicObject + JSON Merge Patch to status subresource -->
<!-- Leader 守卫：leader_handle.is_none_or(|h| h.is_leader()) -->
<!-- 变更检测：比较序列化 JSON 字符串 -->

### Workqueue 结构

<!-- TODO: VecDeque + Mutex + pending set HashMap + Notify + WorkqueueConfig -->

### 410 Gone 处理

<!-- TODO: 检测 watcher 重连，触发完整 KubernetesController 重建 -->
