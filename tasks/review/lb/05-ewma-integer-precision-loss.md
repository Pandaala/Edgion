# [P2] EWMA 整数运算精度损失

## 问题描述

负载均衡 EWMA（指数加权移动平均）延迟在 `runtime_state::update_ewma` 中使用 **百分制整数 alpha** 与 **单次 `/ 100` 截断除法** 更新 `AtomicU64`。该实现会在每一步丢弃余数，无法表示典型生产中的细粒度平滑因子；初始值固定为 **1ms（1000 µs）**，与真实 RTT 量级脱节时会影响新后端的相对排序。`alpha` 由全局原子变量提供，**所有 service 共用同一平滑参数**。

上述问题不改变算法大类（仍是 EWMA 形态），但会引入 **可预期的数值偏差与可观测的调度偏置**，属于可修复的质量类缺陷，而非瞬时 bug。

## 影响范围

| 维度 | 说明 |
|------|------|
| **功能** | 使用 `lb_policy` 中 **EWMA** 选路策略的请求；依赖 `runtime_state::{update_ewma, get_ewma}` 的路径。 |
| **模块** | 核心逻辑在 `src/core/gateway/lb/runtime_state/mod.rs`；`alpha` 读写位于 `src/core/gateway/lb/ewma/metrics.rs`，由 `update_ewma` 通过 `super::ewma::get_alpha()` 拉取。 |
| **数据** | 每个 `(service_key, backend SocketAddr)` 一条 `AtomicU64` EWMA（µs）；无后端条目时 `get_ewma` 回退到同一初始常量。 |

## 严重程度

- **定级：P2（中等）**  
  - 不直接导致崩溃或内存安全错误。  
  - 在 **多后端、延迟接近、依赖 EWMA 细粒度区分** 的场景下，整数截断与粗粒度 alpha 会 **系统性压低或抬高** 某些后端的相对 EWMA，从而影响公平性与“最轻载”语义。  
  - 固定 **1ms** 初始值在 **百毫秒级 RTT** 集群中会使 **新上线/新插入条目的后端** 在收敛前被 **过度偏好**。  

若业务主要使用 RR/LeastConn/CH 而 EWMA 仅为次要策略，现场体感可能较弱；一旦 EWMA 为关键路径，建议优先排期改进。

## 代码位置

### 1. EWMA 更新与初始值（整数公式 + CAS）

```156:176:/Users/caohao/ws1/Edgion/src/core/gateway/lb/runtime_state/mod.rs
/// Initial EWMA value for new backends (1 ms in µs).
const INITIAL_EWMA_US: u64 = 1_000;

/// Update EWMA for `(service_key, addr)`.
pub fn update_ewma(service_key: &str, addr: &SocketAddr, latency_us: u64) {
    let alpha = super::ewma::get_alpha();
    let svc = get_or_create_service(service_key);
    let entry = svc
        .ewma
        .entry(addr.clone())
        .or_insert_with(|| AtomicU64::new(INITIAL_EWMA_US));

    let mut current = entry.load(Ordering::Relaxed);
    loop {
        let new_ewma = (alpha as u64 * latency_us + (100 - alpha as u64) * current) / 100;
        match entry.compare_exchange_weak(current, new_ewma, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(actual) => current = actual,
        }
    }
}
```

### 2. 读取时的默认 EWMA

```178:184:/Users/caohao/ws1/Edgion/src/core/gateway/lb/runtime_state/mod.rs
/// Get EWMA for `(service_key, addr)`.
#[inline]
pub fn get_ewma(service_key: &str, addr: &SocketAddr) -> u64 {
    get_service(service_key)
        .and_then(|s| s.ewma.get(addr).map(|v| v.load(Ordering::Relaxed)))
        .unwrap_or(INITIAL_EWMA_US)
}
```

### 3. 全局 alpha（0–100，默认 20）

```8:28:/Users/caohao/ws1/Edgion/src/core/gateway/lb/ewma/metrics.rs
/// Alpha parameter for EWMA calculation (0-100, representing 0.0-1.0)
/// Default: 20 (represents 0.2)
static ALPHA_PERCENT: AtomicU32 = AtomicU32::new(20);
// ...
pub fn set_alpha(alpha: u32) {
    let clamped = alpha.clamp(0, 100);
    ALPHA_PERCENT.store(clamped, Ordering::Relaxed);
    // ...
}

pub fn get_alpha() -> u32 {
    ALPHA_PERCENT.load(Ordering::Relaxed)
}
```

## 数值分析示例

### 1. 整数除法截断（alpha = 20）

公式实现为：

\[
\text{new} = \left\lfloor \frac{\alpha \cdot L + (100-\alpha) \cdot C}{100} \right\rfloor
\]

- **整百对齐**：`current = 1000`, `latency = 1000`  
  \((20 \cdot 1000 + 80 \cdot 1000) / 100 = 1000\)，与实数结果一致。

- **非整百（用户给定示例）**：`current = 1001`, `latency = 999`  
  实数 EWMA：\(0.2 \times 999 + 0.8 \times 1001 = 1000.6\)。  
  整数：\((19980 + 80080) / 100 = 100060 / 100 = 1000\)，**向下偏差约 0.6 µs/步**。  

单步误差看似极小，但 **每请求一次更新即截断一次**；在 **高 QPS、alpha 较大** 时，误差方向以向下取整为主，长期会累积成 **略偏乐观的 EWMA**（相对连续模型），从而影响与相邻后端的比较。

### 2. alpha 粒度限制

当前 `alpha ∈ [0, 100]` 且为整数，**最小非零步长为 1%**（即 \(\alpha = 0.01\)）。  
常见配置如 **\(\alpha = 0.125\)**（对应约 8 样本半衰思路）**无法精确表达**，只能近似为 `12` 或 `13` 等，进一步偏离目标时间常数。

### 3. 初始值 1ms 与真实 RTT

`INITIAL_EWMA_US = 1000`（1ms）。若典型后端 RTT 为 **50ms–200ms**，新后端在 **首次若干次观测前** 的 EWMA 远低于存量后端，选路逻辑可能 **持续偏向新后端**，直到指数平滑“追上”真实延迟分布；滚动发布或频繁扩缩容时更明显。

### 4. CAS、`Ordering::Relaxed` 与并发语义（补充）

- **CAS 重试**：并发更新下失败线程会加载新 `current` 再算，逻辑正确。  
- **理论上的“值相同但历史不同”**：若某时刻 `current` 经多轮更新后 **数值回到旧读数**，一次成功的 CAS 仍可能基于 **过期的样本与过期的 current 组合** 写入；对 EWMA 这种 **软统计** 通常可接受，但与严格串行更新相比存在 **极少量语义偏差**。  
- **`Relaxed`**：不建立与别处的 happens-before；对 **单原子计数器自身** 的 RMW 仍保证原子性。在 **弱内存模型架构** 上，若未来与其它 `Relaxed` 字段组合推导跨字段一致性，需谨慎；**仅论本字段读写**，对 EWMA 影响通常远小于整数截断与 alpha 粒度问题。

### 5. 全局 `get_alpha()`

`metrics::get_alpha()` 为 **进程级单例**，**无法按 service / 路由** 设置不同响应速度（例如高延迟 RPC vs 低延迟缓存）。多租户或异构后端混部时，统一 alpha 往往是 **次优折中**。

## 修改建议

以下可按投入由小到大采纳，可组合使用。

1. **提高内部比例尺度（定点数）**  
   将公式中的除数由 `100` 改为更大的 **`EWMA_SCALE`（如 1000、65536）**，`alpha` 同步改为 **千分比或定点权重**，使每一步余数更小、截断更弱。示例思路（需与 `set_alpha`/`get_alpha` 的对外语义一并设计）：  
   ```rust
   const EWMA_SCALE: u64 = 1000;
   let new_ewma = (alpha as u64 * latency_us + (EWMA_SCALE - alpha as u64) * current) / EWMA_SCALE;
   ```  
   注意：`alpha` 必须与 `EWMA_SCALE` 同一语义，并保证 `alpha ≤ EWMA_SCALE`，避免下溢为负（当前用 `100 - alpha` 亦同）。

2. **浮点 EWMA（若可接受非整数 µs 存储或单独存 scale）**  
   使用 `f64` 计算再 **四舍五入/银行家舍入** 为 `u64` µs；或用 **`AtomicU64` 存 `f64::to_bits`**（需仔细处理 NaN/有序性，并文档化）。适合要与 **标准 EWMA 公式** 对齐的场景。

3. **细化 alpha 配置**  
   - 支持 **千分比（0–1000）** 或 **直接配置 0.0–1.0**（配置层解析为定点）。  
   - 长期可考虑 **per-service（或 per-route）alpha**，与 `service_key` 维度对齐，而非仅全局 `ALPHA_PERCENT`。

4. **可配置或数据驱动的初始 EWMA**  
   - 将 `INITIAL_EWMA_US` 提升为 **配置项**，或与 **集群/服务历史 P99** 挂钩。  
   - 或使用 **第一次真实 `latency_us` 作为种子**（需注意冷启动并发首次写入的竞态，可用 CAS 或“仅当仍为初始哨兵值时替换”策略）。

5. **内存序（可选）**  
   若与其它指标联合做跨字段不变量推理，可将本路径的 `load`/`compare_exchange` 升为 **`Acquire`/`Release`**；在 **仅用于启发式选路** 的前提下，优先级通常 **低于** 定点精度与 alpha 设计。

6. **测试与回归**  
   增加 **定点公式与浮点参考实现** 的单元测试（给定 `alpha`、序列 `latency`，断言误差界）；对 **新后端初始 EWMA** 行为做集成或属性测试，避免回归。

---

**文档结论**：当前实现 **正确实现了“整数版 EWMA”**，但在 **精度、alpha 表达力、冷启动初值与多租户 alpha** 方面存在 **可预期的系统性偏差**；建议以 **定点放大 + alpha 细化** 为低成本第一步，按业务敏感度再考虑 **浮点或 per-service 配置**。
