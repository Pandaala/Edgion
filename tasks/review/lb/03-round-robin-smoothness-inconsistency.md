# [P2] RoundRobinSelector 不支持平滑加权轮询

## 问题描述

网关负载均衡路径中存在两套「加权轮询」语义：`WeightedRoundRobin`（Smooth WRR，平滑加权轮询）与 `RoundRobinSelector`（简单重复索引的伪 WRR）。二者在相同权重向量下会产生截然不同的调度序列。

- **`WeightedRoundRobin`**（`backend_selector/weighted_selector.rs`）在构造时预计算平滑序列，使高权重后端在周期内**交错**出现，而不是连续成块。
- **`RoundRobinSelector`**（`selection/round_robin.rs`）对每个后端按权重 `weight` 次将同一索引压入 `weighted` 数组，再通过全局原子计数器取模遍历；等价于**简单重复**（classic / naive WRR），高权重后端会在一个周期内**连续**被选中多次。

文档与技能说明中常将「加权轮询」与 `WeightedRoundRobin` 关联，但同一产品内 Endpoint 级选择仍使用非平滑实现，容易造成「配置理解」与「实际流量形状」不一致。

## 影响范围

| 层级 | 组件 | 实现 | 说明 |
|------|------|------|------|
| Route / BackendRef | `BackendSelector` → `WeightedRoundRobin` | Smooth WRR | HTTPRoute 多 `backendRef` 间选择；`items` 顺序来自配置解析后的 `valid_items`，**未**按地址排序 |
| Service 内 Endpoint | `runtime_state::get_rr_selector` → `RoundRobinSelector` | 简单重复 WRR | 按 `service_key` 缓存在 `RR_CACHE`；`RoundRobinSelector::build` 对后端 **按 `addr` 排序** 后再展开权重 |

相关调用链要点：

- `RR_CACHE` 与 `get_rr_selector`：`src/core/gateway/lb/runtime_state/mod.rs`（按 Service 缓存 selector，后端变更时需 `invalidate_selector_cache`）。
- `BackendSelector::create_state`：`src/core/gateway/lb/backend_selector/selector.rs`（多后端时 `WeightedRoundRobin::new(items, weights)`）。

因此：**跨 Service 的 backend 选择与同一 Service 下 Endpoint 选择，在「加权」语义上不对齐**；用户若同时依赖两层加权，会观察到与文档示例（Smooth 序列）不一致的 burst 行为。

## 严重程度

**P2（中等）**

- 非功能性正确性 bug：比例仍大致符合权重，**不**改变长期权重比例。
- 在权重悬殊、QPS 高、或对**请求到达时间分布敏感**的场景下，可能放大尾部延迟、连接/队列尖峰，并间接影响依赖「短窗口错误率」的组件（见下节）。

## 代码位置

| 文件 | 角色 |
|------|------|
| `src/core/gateway/lb/selection/round_robin.rs` | `RoundRobinSelector`：排序 + 简单重复展开 + `counter % weighted.len()` |
| `src/core/gateway/lb/backend_selector/weighted_selector.rs` | `WeightedRoundRobin`：`generate_smooth_sequence` + 同模式计数器取模 |
| `src/core/gateway/lb/runtime_state/mod.rs` | `RR_CACHE`、`get_rr_selector` |
| `src/core/gateway/lb/backend_selector/selector.rs` | `BackendSelector` 聚合为 `WeightedRoundRobin` |

### RoundRobinSelector：简单重复加权

```17:42:Edgion/src/core/gateway/lb/selection/round_robin.rs
    pub fn build(backends: &[Backend]) -> Self {
        // ...
        let mut sorted = backends.to_vec();
        sorted.sort_by(|a, b| a.addr.cmp(&b.addr));

        let mut weighted = Vec::with_capacity(sorted.len());
        for (index, b) in sorted.iter().enumerate() {
            for _ in 0..b.weight.max(1) {
                weighted.push(index as u16);
            }
        }

        RoundRobinSelector {
            backends: sorted.into_boxed_slice(),
            weighted: weighted.into_boxed_slice(),
            counter: AtomicUsize::new(0),
        }
    }
```

### WeightedRoundRobin：Smooth WRR 序列生成

```55:102:Edgion/src/core/gateway/lb/backend_selector/weighted_selector.rs
        // Pre-compute smooth sequence using Smooth WRR algorithm
        let sequence = Self::generate_smooth_sequence(&weights);
        // ...
    fn generate_smooth_sequence(weights: &[usize]) -> Vec<u16> {
        let total: usize = weights.iter().sum();
        let total_weight = total as i64;

        let mut current_weights: Vec<i64> = vec![0; weights.len()];
        let effective_weights: Vec<i64> = weights.iter().map(|&w| w as i64).collect();

        let mut sequence = Vec::with_capacity(total);

        for _ in 0..total {
            for (i, cw) in current_weights.iter_mut().enumerate() {
                *cw += effective_weights[i];
            }
            // ... max_idx, sequence.push, current_weights[max_idx] -= total_weight
        }

        sequence
    }
```

## 行为对比分析

### 1. 序列形状（同一权重向量）

- 权重 `[3, 1, 2]`（注释与单测风格示例，见 `weighted_selector.rs` 文档注释）  
  - **Smooth WRR**：典型交错，如 `[0, 2, 0, 1, 2, 0]`（与实现一致）。  
  - **简单重复**：`[0,0,0,1,2,2]` —— 后端 0 连续三次，再切后端 1、后端 2 连续两次。

- 权重 `[5, 1, 1]`（用户描述场景）  
  - **简单重复**：`A,A,A,A,A,B,C` —— 单周期内 A 上出现 **burst**。  
  - **Smooth WRR**：在周期长度 7 内将 5 次 A 与 B、C 交错，burst 显著减弱。

### 2. 使用场景与心理模型

- **Route 层** `BackendSelector` 已使用 Smooth WRR，文档/运维通常按「平滑」理解加权轮询。  
- **Endpoint 层** `RoundRobinSelector` 仍为简单重复；两层叠加时，**仅 Endpoint 层**会在高权重实例上产生块状流量，与 Route 层观感不一致。

### 3. 非平滑的潜在副作用（定性）

- **有状态或资源紧约束后端**：短时间大量请求打到同一 Endpoint，易形成队列/CPU 尖峰。  
- **熔断/错误率统计**：若熔断器按实例、短窗口统计，连续成功或连续失败都集中在同一后端时，可能与「均匀到达」假设下的调参预期不符（是否触发取决于具体熔断实现与阈值，此处为**风险点**而非已证实缺陷）。  
- **与 Smooth WRR 对比**：平滑算法在相同权重下降低「连续命中同一后端」的最大 run length（在周期内），有利于更均匀的瞬时负载。

### 4. 排序策略差异

| 实现 | 是否排序 | 影响 |
|------|----------|------|
| `RoundRobinSelector::build` | 按 `Backend.addr` 排序 | 多副本/多进程间若输入集合一致，**索引顺序确定**，有利于缓存键与行为一致 |
| `WeightedRoundRobin::new` | 不排序，保留 `items` 传入顺序 | Smooth 序列在**权重相等**时的 tie-break 为「较小索引优先」；整体顺序依赖配置/解析顺序 |

若将 Endpoint 层升级为 Smooth WRR，建议在**排序后的**后端列表上生成序列，以保留 `RoundRobinSelector` 现有的确定性；Route 层是否也要统一排序属于产品决策（可能影响现有依赖顺序的部署）。

### 5. 复杂度与内存

两者均在构造时展开长度为 **总权重和** 的 `u16` 序列，空间与总权重线性相关；Smooth WRR 构造为 O(总权重 × 后端数)，简单重复为 O(总权重)。升级算法不改变「预计算 + 原子计数器」的 O(1) 选择路径，但构造阶段 CPU 略增。

## 修改建议

1. **推荐：统一加权语义（Endpoint 层采用 Smooth WRR）**  
   - 将 `RoundRobinSelector::build` 中「按权重重复 push 索引」替换为与 `WeightedRoundRobin::generate_smooth_sequence` **相同算法**生成的序列；**保留**对 `sorted`（按 `addr`）的排序，再在排序后的列表上计算权重向量。  
   - 行为与 Route 层 `WeightedRoundRobin` 对齐，减少用户与文档认知偏差。

2. **推荐：抽取共用实现，避免双份维护**  
   - 例如将 `generate_smooth_sequence(weights: &[usize]) -> Vec<u16>` 抽到 `lb` 下公共模块（或泛型无关的小工具模块），`WeightedRoundRobin` 与 `RoundRobinSelector` 共用；单元测试集中覆盖边界（权重为 0 已在上层过滤、全 1、极大比值、tie 等）。  
   - `RoundRobinSelector` 仍需保留 `Backend` 特化、`select` 的健康过滤与 fallback 逻辑，仅替换序列生成部分。

3. **可选：文档与可观测性**  
   - 在架构/网关文档中明确两层 LB 的算法；若短期不改代码，至少注明 Endpoint 级为「非平滑 WRR」。  
   - 若需验证线上形状，可在压测下对比升级前后单 Endpoint QPS 的滑动方差或 max run length。

4. **可选：`WeightedRoundRobin` 排序**  
   - 若要求「凡加权轮询均按地址稳定序」，可在 `BackendSelector::create_state` 合并 `items`/`weights` 后按稳定键排序再调用 `WeightedRoundRobin::new`；**属行为变更**，需评估是否破坏依赖声明顺序的存量配置。

## 小结

`RoundRobinSelector` 与 `WeightedRoundRobin` 在「加权轮询」名称下实现不同：`RoundRobinSelector` 为简单重复索引，会在高权重后端上产生 burst；`WeightedRoundRobin` 为 Smooth WRR。二者服务 Route 与 Endpoint 不同层级，但语义分裂易误导运维与调参。优先建议在**排序后的 Endpoint 列表**上采用与 `WeightedRoundRobin` 相同的平滑序列生成，并**抽取共享实现**以降低长期维护成本。
