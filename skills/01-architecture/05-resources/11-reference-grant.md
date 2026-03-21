---
name: resource-reference-grant
description: ReferenceGrant 资源：跨命名空间引用授权、不同步到 Gateway、事件分发 requeue。
---

# ReferenceGrant 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

ReferenceGrant 是 Gateway API 标准资源，用于授权跨命名空间的资源引用。它属于 **no_sync_kind**，不同步到 Gateway，仅在 Controller 侧维护全局授权存储，并在变更时通过事件分发触发受影响资源的重新校验。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs`
- 授权存储: `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_grant/`
- 类型定义: `src/types/resources/reference_grant.rs`

## 不同步到 Gateway

ReferenceGrant 被列入 `DEFAULT_NO_SYNC_KINDS`（`["ReferenceGrant", "Secret"]`），不通过 gRPC 同步到 Gateway。跨命名空间引用的授权决策完全在 Controller 侧完成，结果以 `ref_denied` 字段的形式嵌入路由资源同步到 Gateway。

## Controller 侧处理

### parse

将 ReferenceGrant 写入全局 `ReferenceGrantStore`（upsert）。Store 提供 `check_reference_allowed()` 方法，供其他 Handler 检查跨命名空间引用是否被允许。

### on_change

1. 收集受影响的命名空间集合（包括 ReferenceGrant 所在的 to namespace 和所有 from namespaces）
2. 通过 `ReferenceGrantChangedEvent` 分发变更事件
3. 事件监听器（`CrossNsRefManager`）收到事件后，requeue 所有在受影响命名空间中有跨命名空间引用的资源

### on_delete

1. 从 ReferenceGrantStore 删除该条目
2. 收集受影响命名空间并分发变更事件，触发重新校验

## ReferenceGrant 授权检查流程

其他资源的 Handler 在 parse 阶段检查跨命名空间引用时：

1. 检测 backendRef 或 secretRef 的 namespace 与资源本身的 namespace 不同
2. 调用 `is_cross_ns_ref_allowed()` → `ReferenceGrantStore.check_reference_allowed()`
3. 检查参数：from_namespace、from_group、from_kind、to_namespace、to_group、to_kind、to_name
4. 若不允许：在 backendRef 上设置 `ref_denied` 字段（Gateway 侧据此拒绝请求），或在 Gateway TLS 引用中设置 RefNotPermitted 错误

可通过配置禁用 ReferenceGrant 校验（`is_reference_grant_validation_enabled()`），此时所有跨命名空间引用默认允许。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| ReferenceGrant → HTTPRoute | HTTPRoute | CrossNsRefManager + 事件分发 | 变更时 requeue 有跨命名空间 backendRef 的路由 |
| ReferenceGrant → GRPCRoute | GRPCRoute | CrossNsRefManager + 事件分发 | 同上 |
| ReferenceGrant → TCPRoute | TCPRoute | CrossNsRefManager + 事件分发 | 同上 |
| ReferenceGrant → TLSRoute | TLSRoute | CrossNsRefManager + 事件分发 | 同上 |
| ReferenceGrant → UDPRoute | UDPRoute | CrossNsRefManager + 事件分发 | 同上 |
| ReferenceGrant → Gateway | Gateway | CrossNsRefManager + 事件分发 | 变更时 requeue 有跨命名空间 certificateRef 的 Gateway |
