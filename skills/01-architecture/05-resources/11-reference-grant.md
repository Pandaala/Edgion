---
name: resource-reference-grant
description: ReferenceGrant 资源：跨命名空间引用授权、不同步到 Gateway、CrossNamespaceRefManager。
---

# ReferenceGrant 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 授权跨命名空间引用（Gateway API 标准资源）
- **不同步到 Gateway**
- 仅在 Controller 侧用于校验跨命名空间引用的合法性
-->

### Controller 侧处理

<!-- TODO:
- ReferenceGrantHandler
- ReferenceGrantStore: 中央存储
- CrossNamespaceValidator: 校验跨命名空间引用
- ReferenceGrant 变更时触发 requeue 受影响的资源
- RevalidationListener: 监听变更并重新校验路由
-->

### 跨资源关联

<!-- TODO:
- ← HTTPRoute/GRPCRoute: 跨命名空间的 backendRefs
- ← Gateway: 跨命名空间的 certificateRefs
- ← EdgionPlugins: 跨命名空间引用
-->
