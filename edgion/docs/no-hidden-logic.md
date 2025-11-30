# Edgion 隐藏逻辑文档

> **目的**: 记录代码中的隐藏逻辑、隐式行为和重要设计决策，避免未来产生混淆或意外行为。

**最后更新**: 2025-11-30

---

## 目录

- [#1 EdgionService 类型系统与后端路由](#1-edgionservice-类型系统与后端路由)
- [#2 BackendRefs Weight 字段默认值与权重分配](#2-backendrefs-weight-字段默认值与权重分配)

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

## #2 BackendRefs Weight 字段默认值与权重分配

**位置**: 
- `edgion/src/core/routes/match_engine/regex_routes_engine.rs:56`
- `edgion/src/core/routes/routes_mgr.rs:53`

**日期**: 2025-11-30

### 背景

在 Gateway API 的 HTTPRoute 配置中，`BackendRefs` 用于定义后端服务列表及其权重分配，用于负载均衡。`weight` 字段控制流量分配比例，但该字段是可选的，需要合理的默认值处理。

**Gateway API 关联**：该逻辑对应 [Kubernetes Gateway API](https://gateway-api.sigs.k8s.io/) 规范中 `HTTPRoute.spec.rules[].backendRefs[].weight` 字段。

### 实现细节

在初始化后端选择器（`backend_finder`）时，系统会处理每个 `BackendRef` 的权重：

```rust
// Default weight to 1 if not specified
let weights: Vec<Option<i32>> = refs.iter().map(|br| br.weight.or(Some(1))).collect();
```

**关键行为**：
- 如果 `BackendRef.weight` 为 `None`（未指定），则自动设置为 `Some(1)`
- 如果显式指定了权重值（包括 0），则使用指定的值
- 权重会传递给负载均衡器进行流量分配

### ⚠️ 隐藏逻辑

#### 1. 默认权重为 1
- **未指定 `weight` 字段时，系统自动设置权重为 1**
- 这确保了所有后端在没有明确权重配置时获得相等的流量分配
- 符合常见的负载均衡配置习惯

#### 2. 权重为 0 的特殊用途
- **权重可以显式设置为 0**
- **权重为 0 的后端不会接收任何流量**
- **可用于蓝绿发布、金丝雀发布等场景**

**典型使用场景**：

```yaml
# 场景 1: 蓝绿发布 - 只有蓝环境接收流量
backendRefs:
  - name: blue-service
    weight: 1        # 接收所有流量
  - name: green-service
    weight: 0        # 不接收流量（待切换）

# 场景 2: 金丝雀发布 - 10% 流量到新版本
backendRefs:
  - name: stable-service
    weight: 9        # 90% 流量
  - name: canary-service
    weight: 1        # 10% 流量

# 场景 3: 未指定权重 - 平均分配
backendRefs:
  - name: backend-1  # 自动权重 1
  - name: backend-2  # 自动权重 1
  - name: backend-3  # 自动权重 1
  # 三个后端各接收 33.3% 流量
```

### 设计理由

1. **默认权重为 1**：
   - 简化配置：用户不需要为每个后端都显式指定权重
   - 直观行为：多个后端默认均匀分配流量
   - 向后兼容：与常见的负载均衡器行为一致

2. **支持权重为 0**：
   - 灵活的流量控制：可以在不删除后端的情况下临时停止流量
   - 渐进式发布：支持蓝绿部署、金丝雀部署等高级发布策略
   - 快速回滚：只需调整权重即可切换流量，无需修改后端列表

3. **权重比例计算**：
   - 负载均衡器根据权重比例分配请求
   - 例如：权重 `[3, 1, 1]` 表示第一个后端接收 60% 流量，其他各 20%
   - 总权重为 0 时会导致错误（`ERR_INCONSISTENT_WEIGHT`）

### 未来考虑事项

- **添加权重验证**：
  * 检测所有权重为 0 的情况并提供友好的错误信息
  * 添加权重配置的验证日志
- **增强可观测性**：
  * 添加指标记录每个后端的实际流量分配比例
  * 记录权重变更事件以便追踪发布过程
- **动态权重调整**：
  * 考虑支持基于健康检查自动调整权重
  * 支持基于性能指标的自适应权重分配

### 相关代码

- **HTTPBackendRef 结构体**: `edgion/src/types/resources/http_route.rs:174-203`
  * 包含 `weight` 字段作为 `Option<i32>`
  * 对应 Gateway API 规范中的 `BackendRef.weight` 字段
  * 在 HTTPRoute 中作为 `spec.rules[].backendRefs[].weight` 使用
- **BackendSelector**: `edgion/src/core/lb/backend_selector.rs`
  * 负责根据权重进行后端选择
  * 实现加权随机负载均衡算法
- **权重初始化位置**：
  * Regex 路由引擎: `regex_routes_engine.rs` 的 `select_backend()` 方法
  * 普通路由管理器: `routes_mgr.rs` 的 `select_backend()` 方法

### Gateway API 规范参考

根据 [Gateway API BackendRef 规范](https://gateway-api.sigs.k8s.io/reference/spec/#gateway.networking.k8s.io/v1.BackendObjectReference)：
- `weight` 字段是可选的 (optional)，类型为 `int32`
- 默认值为 `1`（但需要实现层处理）
- 有效范围：`0` 到 `1000000`
- 权重为 `0` 表示该后端不接收流量
- 权重是相对值，实际流量分配按比例计算

---

## #3 待添加

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

