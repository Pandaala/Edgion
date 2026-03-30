# [P3] Preload 模块 lb_type 参数未实际使用

## 问题描述

`preload.rs` 在预热路径上贯穿传递 `lb_type`（`Option<LbPolicyType>`），并将 `(service_key, lb_type)` 作为 `PreloadKey` 参与去重；但 `preload_eps_lb` / `preload_endpoint_lb` 的实际行为**仅**对 RoundRobin 存储执行 `get_or_create(service_key)`，`lb_type` 只出现在 `tracing::trace!` 字段中，不参与分支或数据结构初始化。

与此同时，文件头部对 `PreloadKey` 的注释声称「相同 service 不同 LB 策略需要不同的 LB 实例」，与当前实现（只预热 RR 数据层、其他策略在选取时读 RR 列表）**语义不一致**，容易让读者误以为预热会按策略分实例或分路径执行。

## 影响范围

- **模块**：`src/core/gateway/backends/preload.rs`（LB 启动预热）。
- **入口**：`preload_load_balancers` 由 `src/core/gateway/cli/mod.rs` 在网关就绪流程中调用（与 `ConfigClient` 上的路由列表配合）。
- **数据流**：HTTP/gRPC/UDP/TLS 等 `collect_*_routes` 从 `extension_info.lb_policy` 解析并写入 `PreloadKey`；TCP 路由无 extension，固定 `None`。
- **外部契约**：无公开 API 暴露 `LbPolicyType`（模块内 `enum`），但日志指标 `total` / `success` / `skipped` 的计数会因「同 service、不同 lb_type」而**人为膨胀**，可能被运维误判为「预热了更多 distinct LB」。

## 严重程度

| 维度 | 说明 |
|------|------|
| **级别** | **P3**（接口/注释误导与轻微无效工作；非线上功能错误） |
| **正确性** | 重复调用 `get_or_create` 对同一 `service_key` 仍为幂等，一般不破坏行为。 |
| **可维护性** | 新同学易按参数名与 `PreloadKey` 注释实现「按策略预热」，与真实架构脱节。 |
| **性能** | 同一 backend service 被多种 LB 策略引用时，预热循环次数增加，多为重复 `get_or_create` + trace，通常远小于路由总数，但在超大路由集下仍有边际成本。 |

## 代码位置

| 项 | 文件 | 大致行号 |
|----|------|----------|
| `PreloadKey` 类型别名与注释 | `preload.rs` | 12–14 |
| 内联 `LbPolicyType` 与 `From<ParsedLBPolicy>` | `preload.rs` | 16–31 |
| `preload_load_balancers` 聚合与循环 | `preload.rs` | 38–69 |
| `collect_http_routes` / `collect_grpc_routes` / `collect_udp_routes` / `collect_tls_routes` | `preload.rs` | 72–174 |
| `collect_tcp_routes`（无 lb_policy） | `preload.rs` | 114–132 |
| `preload_lb` / `preload_eps_lb` / `preload_endpoint_lb` | `preload.rs` | 177–232 |
| 对外导出与 CLI 调用 | `backends/mod.rs`、`cli/mod.rs` | `pub use`、约 250 行 |

## 详细分析

### 1. `preload_lb` 接收 `lb_type` 但未参与控制流

`preload_lb` 仅按 `EndpointMode` 分发到 EPS 或 Endpoint 的 RR 存储；两处实现路径上 `lb_type` 均未用于条件判断或初始化不同结构体。

```177:181:/Users/caohao/ws1/Edgion/src/core/gateway/backends/preload.rs
fn preload_lb(service_key: &str, lb_type: &Option<LbPolicyType>, mode: EndpointMode) -> bool {
    match mode {
        EndpointMode::EndpointSlice | EndpointMode::Both | EndpointMode::Auto => preload_eps_lb(service_key, lb_type),
        EndpointMode::Endpoint => preload_endpoint_lb(service_key, lb_type),
    }
}
```

`preload_eps_lb` 中注释明确说明 LeastConn / EWMA / ConsistentHash 在选取阶段读 RR 的后端列表，因此预热只需 RR；`lb_type` 仅写入 trace：

```184:207:/Users/caohao/ws1/Edgion/src/core/gateway/backends/preload.rs
fn preload_eps_lb(service_key: &str, lb_type: &Option<LbPolicyType>) -> bool {
    use crate::core::gateway::backends::discovery::endpoint_slice::get_roundrobin_store;

    let rr_store = get_roundrobin_store();

    // Preload the RoundRobin LB which owns the data layer.
    // LeastConn/EWMA/ConsistentHash read from the RR backend list at
    // selection time, so no separate LB instances needed.
    let rr_lb = rr_store.get_or_create(service_key);
    // ...
    tracing::trace!(
        component = "lb_preload",
        service_key,
        lb_type = ?lb_type,
        "Preloaded LB (EndpointSlice)"
    );
```

`preload_endpoint_lb` 对称，同样只有 trace 使用 `lb_type`。

**结论**：从「行为」上看，`lb_type` 是当前实现中的**死参数**（dead parameter），仅服务于可选的可观测性字段。

### 2. `PreloadKey` 包含 `lb_type` 导致去重粒度与真实预热单位不一致

```12:14:/Users/caohao/ws1/Edgion/src/core/gateway/backends/preload.rs
/// Key for deduplication: (service_key, lb_policy_type)
/// Same service with different LB policies need separate LB instances
type PreloadKey = (String, Option<LbPolicyType>);
```

去重键若为 `(service_key, lb_type)`，则同一 `namespace/name` 的 backend 在 HTTPRoute A 上配置 ConsistentHash、在 HTTPRoute B 上配置 LeastConn 时，`HashSet` 会保留**两条**键。循环内两次调用 `preload_lb` 最终都执行同一 `get_or_create(service_key)`，第二次在 RR 已存在时等价于 no-op（视 `get_or_create` 实现可能仍有轻量检查）。

**结论**：

- 注释「需要 separate LB instances」对**预热模块**而言不准确：预热并未创建按策略区分的实例。
- 去重精度**高于**实际有意义的预热单位（`service_key`），带来冗余迭代与可能误导的 `total` 统计。

### 3. 注释与实现矛盾，接口具有误导性

设计意图在 `preload_eps_lb` 内已有清晰说明（RR 拥有数据层，其余策略共享其视图）。但：

- `PreloadKey` 文档仍暗示「按策略分实例」；
- `preload_load_balancers` 中注释写「Preload each unique (service_key, lb_policy)」，暗示策略维度有语义，与「只预热 RR」不一致。

这种**文档层**与**实现层**的割裂，比「多几次循环」更难排查：读者会合理推断未来或他处存在按 `lb_type` 分支的预热逻辑，从而错误修改或依赖该字段。

### 4. `collect_*` 收集了在预热路径上未使用的行为信息

除 TCP（无 extension）外，各 `collect_*_routes` 均从 `br.extension_info.lb_policy` 映射为 `LbPolicyType` 并插入 `keys`：

```84:88:/Users/caohao/ws1/Edgion/src/core/gateway/backends/preload.rs
                let key = format!("{}/{}", ns, br.name);
                let lb_type = br.extension_info.lb_policy.as_ref().map(LbPolicyType::from);
                keys.insert((key, lb_type));
```

这些信息在后续**唯一**消费点是 trace 与去重键的第二分量；对 RR `get_or_create` 无输入作用。若移除 `lb_type`，collect 侧可简化为仅 `service_key`，减少与配置结构的耦合。

### 5. 与观测指标的交叉影响

`preload_load_balancers` 用 `keys.len()` 作为 `total`，用 `preload_lb` 返回值累计 `success` / `skipped`。由于同一 `service_key` 可对应多条 `PreloadKey`，`total` 不等于「已预热的 distinct service 数」。在排障时若将 `total` 解释为「backend 数」或「LB 实例数」，会产生偏差。

## 修改建议

### 方案 A（推荐）：简化键与签名，与实现一致

- 将 `PreloadKey` 改为 `String`（或 `type PreloadKey = String`），`HashSet<PreloadKey>` 仅按 `service_key` 去重。
- 删除 `preload_lb` / `preload_eps_lb` / `preload_endpoint_lb` 的 `lb_type` 参数；`collect_*` 不再读取 `lb_policy`（TCP 与带 extension 的路径统一为插入 `key`）。
- **修正**模块内注释：明确预热目标是「RR 存储中的后端列表与 LB 句柄」，非「每种 LB 策略一个预热目标」。
- **收益**：更少遍历次数、更清晰心智模型、更低与 `ParsedLBPolicy` 的耦合。

### 方案 B：保留形状，标注为未来扩展点

- 若产品明确规划「按策略差异化预热」（见方案 C），保留 `(service_key, lb_type)` 或保留参数，但应：
  - 重写 `PreloadKey` 注释，说明当前**不**按策略分实例，仅预留或仅用于观测；
  - 在 `preload_lb` 处加 `TODO`/`NOTE` 指向设计文档或 issue，避免读者误以为已实现分支逻辑。

### 方案 C：让接口名副其实（长期架构项）

- 若确需降低首包延迟，可在预热阶段按 `lb_type` 做真实工作，例如：
  - ConsistentHash：预构建 ring 或预拉取与 slice 同步的哈希状态（需与现有 selector 存储对齐）；
  - EWMA / LeastConn：预分配统计结构或做一次 dry-run 选取（需评估是否有副作用与线程模型）。
- 此方案工作量大，需与 `endpoint_slice` / selector 层设计一并评审，不适合作为「仅删参数」的同一变更。

### 综合建议

短期优先 **方案 A**，与现有注释（RR 为数据层、其他策略读 RR）完全一致；若团队已排期差异化预热，可过渡为 **方案 B** 并在实现落地后演进到 **方案 C**，避免长期维持「参数存在但无行为」的状态。

---

**审查基准**：仓库内 `preload.rs` 当前实现（见上文引用行号）；调用关系以 `preload_load_balancers` 的 `rg` 结果为准。
