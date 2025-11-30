# Edgion 隐藏逻辑文档

> **目的**: 记录代码中的隐藏逻辑、隐式行为和重要设计决策，避免未来产生混淆或意外行为。

**最后更新**: 2025-11-30

---

## 目录

- [#1 EdgionService 类型系统与后端路由](#1-edgionservice-类型系统与后端路由)

---

## #1 EdgionService 类型系统与后端路由

**位置**: `edgion/src/core/backends/mod.rs`

**日期**: 2025-11-30

### 背景

系统需要支持多种类型的 Kubernetes 后端服务，但当前只有标准的 Service 类型被完全实现，并且应该被处理用于端点解析。

**Gateway API 关联**：该逻辑对应 [Kubernetes Gateway API](https://gateway-api.sigs.k8s.io/) 规范中 `HTTPRoute.spec.rules[].backendRefs[].kind` 字段。在 Gateway API 中，`BackendRef` 的 `kind` 字段用于指定后端引用的资源类型。

### 实现细节

1. **定义了 EdgionService enum，包含四种类型**：
   ```rust
   pub enum EdgionService {
       Service,              // 标准 Kubernetes Service (默认)
       ServiceClusterIp,     // 带 ClusterIP 的 Service
       ServiceImport,        // 用于多集群的 ServiceImport
       ServiceExternalName,  // 带 ExternalName 的 Service
   }
   ```

2. **添加了 `EdgionService::from_kind()` 方法**：
   - 解析 `HTTPBackendRef.kind` 字段 (`Option<String>`)
   - 该字段对应 Gateway API 中的 `BackendRef.kind`
   - 在以下情况返回 `Service` 类型：
     * `kind` 为 `None`（未指定，符合 Gateway API 的默认行为）
     * `kind` 为空字符串 `""`
     * `kind` 为未知/未识别的值
   - 这确保了向后兼容性和安全的默认行为

3. **修改了 `get_peer()` 函数**：
   - 在函数开始时从 `br.kind` 提取服务类型
   - 只处理 Service 类型进行端点解析
   - 对所有其他服务类型返回 `None`
   - 这防止了对不支持的服务类型进行错误的路由

### ⚠️ 隐藏逻辑

- **只有 Service 类型的后端会被处理用于 peer 解析**
- **空或缺失的 'kind' 字段默认为 Service（不是错误）**
- **未知的服务类型被当作 Service 处理（故障安全默认）**
- **非 Service 类型静默返回 None（无错误，无路由）**

### 设计理由

- Service 是最常见且完全支持的 K8s 后端类型
- 其他类型（ServiceImport、ServiceExternalName 等）需要不同的处理逻辑，目前尚未实现
- 默认为 Service 保持了与未指定 kind 字段的现有配置的向后兼容性
- 对不支持的类型返回 None 比尝试错误地处理它们更安全

### 未来考虑事项

- **当实现 ServiceImport/ServiceExternalName 支持时**：
  * 在 `get_peer()` 中添加相应的逻辑分支
  * 更新本文档说明新类型的处理方式
  * 考虑为不支持类型的尝试添加日志/指标
- 可能需要添加显式的错误处理/日志记录，而不是静默返回 None
- 考虑通过 feature flags 使默认行为可配置

### 相关代码

- **HTTPBackendRef 结构体**: `edgion/src/types/resources/http_route.rs:174-203`
  * 包含 `kind` 字段作为 `Option<String>`
  * 对应 Gateway API 规范中的 `BackendRef.kind` 字段
  * 在 HTTPRoute 中作为 `spec.rules[].backendRefs[].kind` 使用
- **MatchInfo 使用**: 传递给 `get_peer()` 携带命名空间信息
- **Endpoint slice store**: 用于 Service 类型的 peer 解析

### Gateway API 规范参考

根据 [Gateway API BackendRef 规范](https://gateway-api.sigs.k8s.io/reference/spec/#gateway.networking.k8s.io/v1.BackendRef)：
- `kind` 字段是可选的，默认值为 `"Service"`
- 支持的标准类型包括：
  * `Service` - 标准 Kubernetes Service（默认）
  * `ServiceImport` - 用于多集群服务发现（来自 MCS API）
- 其他类型如 `ServiceExternalName` 等可能需要特定实现支持

---

## #2 待添加

_预留位置_

---

## #3 待添加

_预留位置_

---

## #4 待添加

_预留位置_

---

## 贡献指南

当添加新的隐藏逻辑条目时：

1. 在目录中添加新条目的链接
2. 按顺序编号（#2、#3、#4...）
3. 包含以下部分：
   - **位置**: 代码文件路径
   - **日期**: 添加日期
   - **背景**: 为什么需要这个逻辑
   - **实现细节**: 具体如何实现
   - **⚠️ 隐藏逻辑**: 重点标注非显式的行为
   - **设计理由**: 为什么这样设计
   - **未来考虑事项**: 后续可能的改进
   - **相关代码**: 相关文件和位置
4. 移除相应的"待添加"占位符

