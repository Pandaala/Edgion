# [P3] select_from_backends 参数过多（9 个参数）

## 问题描述

`select_from_backends` 是 EndpointSlice 与 Endpoints 两条后端发现路径共用的核心选路函数，当前签名包含 **9 个参数**：`service_key`、`lb_policy`、`session`、`backends` 以及 **5 个并列的 `EdgionStatus`**（分别对应「空列表 / RR 默认 / RR / CH / LC / EWMA」分支下的未选到后端错误）。

其中后 5 个参数在语义上并非独立业务输入，而是同一维度——**数据来源（EndpointSlice vs Endpoints）** 与 **负载均衡算法** 的笛卡尔积在错误码上的展开。调用方 `select_from_endpoint_slice` 与 `select_from_endpoints` 的逻辑几乎完全一致，唯一区别是传入的 `BackendEndpointSliceNotFoundBy*` 与 `BackendEndpointNotFoundBy*` 系列枚举值。

这是一种典型的 **Long Parameter List** 代码坏味道：共享实现被迫通过冗长参数表区分本应内聚的「错误上下文」，增加阅读成本与演进时的修改面。

## 影响范围

| 维度 | 说明 |
|------|------|
| **直接模块** | `src/core/gateway/backends/mod.rs`：`select_from_backends`、`select_from_endpoint_slice`、`select_from_endpoints`。 |
| **类型定义** | `src/types/edgion_status.rs`：10 个 `Backend*NotFoundBy*` 变体（500_05–500_14）。 |
| **间接调用** | `select_backend_by_policy` → 上述两路径；`try_get_peer` 中会话粘滞失败时硬编码 `BackendEndpointSliceNotFoundByConsistent`（与「未从 EndpointSlice 选到后端」语义混用，见下文分析）。 |
| **HTTP 映射** | `get_peer` 对大部分 `EdgionStatus` 走 `end_response_503`；这些细粒度错误码主要进入 `ctx.add_error` 等可观测路径，而非一一对应不同 HTTP 状态分支。 |

## 严重程度

| 维度 | 说明 |
|------|------|
| **级别** | **P3**（可维护性与扩展性；非当前功能错误） |
| **正确性** | 现有映射关系清晰，行为正确。 |
| **演进成本** | 每新增一种 `ParsedLBPolicy` 变体，需同步改：本函数签名、两处 `select_from_*` 调用、`EdgionStatus` 枚举，并评估序列化/文档/外部依赖对错误码的兼容性。 |
| **认知负担** | 新读者需理解 5 个 `not_found_*` 与 `match lb_policy` 分支的一一对应关系，易漏改或传错顺序。 |

## 代码位置

| 符号 | 文件 | 行号（约） |
|------|------|------------|
| `select_from_backends` | `backends/mod.rs` | 452–497 |
| `select_from_endpoint_slice` | `backends/mod.rs` | 499–516 |
| `select_from_endpoints` | `backends/mod.rs` | 518–535 |
| `select_backend_by_policy` | `backends/mod.rs` | 437–450 |
| `Backend*NotFoundBy*` 枚举 | `types/edgion_status.rs` | 40–67 |
| 会话粘滞失败分支 | `backends/mod.rs` | 812–825（`try_get_peer`） |

核心实现摘录：

```453:497:/Users/caohao/ws1/Edgion/src/core/gateway/backends/mod.rs
fn select_from_backends(
    service_key: &str,
    lb_policy: &Option<ParsedLBPolicy>,
    session: &Session,
    backends: Vec<pingora_load_balancing::Backend>,
    not_found_rr_default: EdgionStatus,
    not_found_rr: EdgionStatus,
    not_found_ch: EdgionStatus,
    not_found_lc: EdgionStatus,
    not_found_ewma: EdgionStatus,
) -> Result<pingora_load_balancing::Backend, EdgionStatus> {
    if backends.is_empty() {
        return Err(not_found_rr_default);
    }
    // ... match lb_policy: None / ConsistentHash / LeastConn / Ewma，各分支 .ok_or(not_found_*) ...
}
```

两处调用仅错误码族不同：

```505:515:/Users/caohao/ws1/Edgion/src/core/gateway/backends/mod.rs
    select_from_backends(
        service_key,
        lb_policy,
        session,
        backends,
        EdgionStatus::BackendEndpointSliceNotFoundByRoundRobinDefault,
        EdgionStatus::BackendEndpointSliceNotFoundByRoundRobin,
        EdgionStatus::BackendEndpointSliceNotFoundByConsistent,
        EdgionStatus::BackendEndpointSliceNotFoundByLeastConn,
        EdgionStatus::BackendEndpointSliceNotFoundByEwma,
    )
```

```524:534:/Users/caohao/ws1/Edgion/src/core/gateway/backends/mod.rs
    select_from_backends(
        service_key,
        lb_policy,
        session,
        backends,
        EdgionStatus::BackendEndpointNotFoundByRoundRobinDefault,
        EdgionStatus::BackendEndpointNotFoundByRoundRobin,
        EdgionStatus::BackendEndpointNotFoundByConsistent,
        EdgionStatus::BackendEndpointNotFoundByLeastConn,
        EdgionStatus::BackendEndpointNotFoundByEwma,
    )
```

## 详细分析

### 1. 函数签名过长与参数语义重复

9 个参数中，5 个 `EdgionStatus` 的用途单一：在 `backends` 为空或各算法 `select` 返回 `None` 时作为 `Err` 返回。它们与 `lb_policy` 的分支结构强耦合——每增加一个 `ParsedLBPolicy` 变体，就要多一个「未找到」错误槽位。

`not_found_rr_default` 与 `not_found_rr` 的区分来自实现细节：`None` 策略与「一致性哈希但 hash key 为空」时都会走 RR，但使用不同的错误码变体（500_08 vs 500_07 或 500_13 vs 500_12）。这对排障可能有价值，但不应通过五个平铺参数表达。

### 2. 调用者差异仅在于错误码：应内聚到被调函数或类型

`select_from_endpoint_slice` 与 `select_from_endpoints` 的差异只有：

- `get_roundrobin_store()` vs `get_endpoint_roundrobin_store()` 取 `backends`；
- 传入的 `EdgionStatus` 前缀 `BackendEndpointSlice*` vs `BackendEndpoint*`。

这说明 **「数据来自哪种 K8s 资源」** 才是调用者真正想表达的配置，而不是五段独立的 status。更自然的 API 是传入 `BackendDiscoverySource`（或复用文件中已有的 `PortLookupSource` 思想：`EndpointSlice` / `Endpoint`），在 `select_from_backends` 内部（或紧邻的小函数中）映射到具体 `EdgionStatus`。

### 3. 扩展性差与修改面扩散

新增一种 LB 算法（例如加权、随机、区域优先）时，典型改动链为：

1. 扩展 `ParsedLBPolicy` 与 `select_from_backends` 的 `match`；
2. 为 `select_from_backends` 增加第 6 个 `not_found_*` 参数（签名变为 10 个参数）；
3. 在两个 `select_from_*` 调用点各补一个 `EdgionStatus` 变体；
4. 在 `EdgionStatus` 中增加两个变体（Slice + Endpoint 各一），并更新序列化/文档/可能的对外错误码表。

若采用 **来源 × 算法** 的组合模型（见修改建议），则新增算法时往往只需：一处 `match` 分支 + 枚举扩展（或单变体带字段），调用点参数个数可保持稳定。

### 4. 错误码组合爆炸与消费端是否区分使用

当前组合为 **2（EndpointSlice / Endpoints）× 5（RR default、RR、CH、LC、EWMA）= 10** 个独立变体。再加一种算法则为 12 个。

仓库内检索表明：这些变体除在 `edgion_status.rs` 定义、`backends/mod.rs` 赋值外，**没有**其它模块对具体变体做 `match` 分支；`try_get_peer` 中会话粘滞目标不健康时直接返回 `BackendEndpointSliceNotFoundByConsistent`，与「EndpointSlice + CH 路径未选到后端」共用同一码，说明 **并非所有失败路径都严格按「来源×算法」建模**。

`EdgionStatus` 实现 `serde::Serialize`，会进入错误上下文；若运维或 API 消费者主要按**数值码**或**枚举名**区分告警，则细粒度码有价值。若实际只区分「无后端 / 503」级别，则存在 **过度枚举** 的可能，可考虑收敛为带字段的单变体，在日志/指标里展开 `source` 与 `algorithm`。

### 5. 与 `get_peer` 的 HTTP 行为关系

`get_peer` 将绝大多数后端解析错误统一映射为 **503**（少数配置类错误为 500）。因此这 10 个码对 **HTTP 响应形态** 的区分度有限，主要价值在 **可观测性与排障语义**。重构时若合并变体，需评估是否依赖稳定错误码的下游（监控规则、工单系统、客户集成文档）。

## 修改建议

### 方案 A：错误码封装为结构体（最小侵入）

将五个状态收拢为单一参数，例如：

```rust
struct BackendSelectionNotFoundStatuses {
    rr_default: EdgionStatus,
    rr: EdgionStatus,
    ch: EdgionStatus,
    lc: EdgionStatus,
    ewma: EdgionStatus,
}
```

`select_from_backends(..., errors: BackendSelectionNotFoundStatuses)` 将参数从 9 个降为 **5 个**，调用处通过两个常量或构造函数分别构造 Slice 版与 Endpoint 版，避免签名继续膨胀。

**优点**：改动面小，不改变 `EdgionStatus` 对外枚举。  
**缺点**：新增算法仍要改结构体字段与两处构造。

### 方案 B：传入数据来源枚举，在内部映射错误码（推荐方向）

定义例如：

```rust
#[derive(Clone, Copy)]
enum BackendListSource {
    EndpointSlice,
    Endpoint,
}
```

`select_from_backends(service_key, lb_policy, session, backends, source)` 内部根据 `source` 与当前 `match` 分支调用 `fn not_found(source, algorithm_kind) -> EdgionStatus`（或 `match (source, policy_branch)`）。

**优点**：调用点极薄，与「两调用者仅差错误码族」的事实一致；新增算法时集中改映射表。  
**缺点**：需在单文件或小组件内维护映射函数，注意与 `EdgionStatus` 命名一致。

### 方案 C：收敛错误模型（长期）

若确认无下游依赖 10 个独立码，可引入：

```rust
BackendNotFound {
    source: BackendListSource,
    algorithm: LbAlgorithm, // 含 RrDefault / Rr / Ch / Lc / Ewma 等
}
```

或保留单一 `EdgionStatus::BackendNotFound` + 结构化上下文（日志字段、metrics label）。**需兼容性评估**（序列化格式、已有告警规则）。

### 顺带：`try_get_peer` 中的硬编码

会话粘滞目标不健康时返回 `BackendEndpointSliceNotFoundByConsistent`（`mod.rs` 约 824 行），在全局 `EndpointMode::Endpoint` 时语义上并不严格对应「EndpointSlice + 一致性哈希」。重构时建议改为与当前实际选路来源一致的状态，或统一为更中性的 `BackendLoadBalancerSelectionFailed`（若已有）并附带结构化原因。

## 重构对比

| 对比项 | 现状 | 方案 A（结构体） | 方案 B（source + 内部映射） | 方案 C（组合变体） |
|--------|------|------------------|-----------------------------|---------------------|
| `select_from_backends` 参数个数 | 9 | 5 | 5 | 5（或 4 + 组合错误类型） |
| 新增 LB 算法时需改调用点行数 | 2 处 × 5 参数之一 | 2 处结构体初始化 | 通常 0；改映射表 | 枚举/上下文扩展 |
| `EdgionStatus` 变体数量 | 10 个独立 | 不变 | 不变 | 可减少 |
| 与调用语义一致性 | 参数表重复表达「来源」 | 中等 | 高 | 高 |
| 对外错误码兼容性 | — | 完全兼容 | 完全兼容 | 可能需迁移 |

## 小结

`select_from_backends` 的 9 参数签名是 **共享逻辑 + 双数据源 + 多算法错误码** 叠加的结果，本身正确，但在可读性与扩展性上代价偏高。优先建议采用 **方案 B** 将「EndpointSlice / Endpoint」内聚为单一 `source` 参数并在内部映射到现有 `EdgionStatus`；若短期内只求降低签名噪音，**方案 A** 即可落地。若可接受破坏性评估，**方案 C** 能从根本上抑制错误码组合爆炸。同时应复核 `try_get_peer` 中粘滞失败所用状态与实际 endpoint 模式是否一致。
