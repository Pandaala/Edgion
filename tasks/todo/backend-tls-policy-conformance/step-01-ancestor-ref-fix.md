# Step 01: ancestorRef 指向 Gateway

> **Gap 1（致命）**：ancestorRef 当前指向 BackendTLSPolicy 自身，conformance 要求指向 Gateway。

## 现状

`backend_tls_policy.rs` 第 203-210 行构建了 synthetic ancestor：

```rust
kind: Some("BackendTLSPolicy".to_string()),  // 错误
name: btp.metadata.name...,                   // 错误
```

## Conformance 要求

测试 helper `BackendTLSPolicyMustHaveCondition` 第 1429 行检查：

```go
parent.AncestorRef.Name == gatewayv1.ObjectName(gwNN.Name)
```

即 ancestorRef 必须指向 Gateway 对象（如 `same-namespace`）。

## 修复方案

从 BackendTLSPolicy 的 targetRef 反向查找关联的 Gateway：

```
BackendTLSPolicy.targetRef -> Service -> HTTPRoute.parentRef -> Gateway
```

需要在 handler context 中：
1. 拿到 targetRef 指向的 Service
2. 遍历所有 HTTPRoute，找到 backendRef 引用该 Service 的路由
3. 从这些 HTTPRoute 的 parentRef 找到关联的 Gateway(s)
4. 为每个 Gateway 生成一条 ancestor status

## 关键问题

- handler context 中是否已有 route-to-gateway 映射？需要确认现有 store 索引
- 一个 BackendTLSPolicy 可能关联多个 Gateway，status 需要多条 ancestor entries

## 验证

- `BackendTLSPolicyInvalidKind` 测试会检查 ancestorRef
- `BackendTLSPolicy` 主测试也会检查
