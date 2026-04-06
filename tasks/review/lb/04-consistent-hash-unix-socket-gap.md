# [P3] ConsistentHashRing 不支持 Unix Domain Socket

---

## 问题描述

当上游后端地址为 **Unix Domain Socket（UDS）** 时，`ConsistentHashRing::build` 在构造 ketama 环与 `addr_to_idx` 映射时**仅接受** `pingora_core::protocols::l4::socket::SocketAddr::Inet`，对 `SocketAddr::Unix` 分支返回 `None`，导致：

1. UDS 后端**不会进入** `pingora_ketama::Continuum` 的 bucket 列表，不参与一致性哈希。
2. 同一套过滤逻辑使 `addr_to_idx` 中**不存在** UDS 后端的索引映射。

运行时表现为：**配置里看似有多个后端，但一致性哈希路径上只有 TCP/IP 后端生效**；若全部为 UDS，环可能为空或仅剩部分后端，且**无任何日志**说明被丢弃的原因。

此外，**会话持久化（Session Persistence）** 相关的 affinity 表构建逻辑同样只处理 `Inet`，与一致性哈希存在**同构缺陷**。

---

## 影响范围

| 维度 | 说明 |
|------|------|
| **直接模块** | 网关负载均衡：`ConsistentHashRing`、按服务缓存的 CH 环（`get_ch_ring`）、SP affinity 映射 |
| **典型场景** | Kubernetes Service 以 ClusterIP/Endpoint IP 为主时影响较小；**边缘 / sidecar 本地 UDS**、**本地开发**将后端指向 socket 文件时影响显著 |
| **与其他算法对比** | `RoundRobinSelector::build` 对 `Backend` 全量参与（按 `addr` 排序），**不**按 Inet/Unix 过滤，UDS 在 RR 下仍可选中；与 CH/SP 行为**不一致** |
| **可观测性** | 当前为**静默丢弃**，易出现「以为按 hash 分散、实际全打到少数 Inet 后端」或「SP token 无法对应 UDS」等**难排查**问题 |

---

## 严重程度

**P3（低～中，依部署形态而定）**

- **默认 K8s 集群内 Service**：多为 `Inet`，问题**不易触发**。
- **一旦使用 UDS 作为 Backend**：一致性哈希与 SP affinity **语义错误**且**无告警**，对正确性与运维体验的影响可升至**中**；故整体标为 **P3**，但在 UDS 部署下应按 **P2 级别** 对待修复与文档说明。

---

## 代码位置

| 文件 | 职责 |
|------|------|
| `Edgion/src/core/gateway/lb/selection/consistent_hash.rs` | Ketama 环构建、`addr_to_idx`、`select` 中与环地址的反向查找 |
| `Edgion/src/core/gateway/lb/runtime_state/mod.rs` | `get_ch_ring` 缓存；`sp_affinity_build` / `sp_affinity_get_or_build`（CRC32 token → `std::net::SocketAddr`） |
| `Edgion/src/core/gateway/lb/selection/round_robin.rs` | 对比参考：未按 Unix 过滤后端 |

---

## 详细分析

### 1. `build()` 仅将 `SocketAddr::Inet` 加入 ketama buckets

`Bucket::new` 接收的是从 `Inet` 解包出的 `std::net::SocketAddr`；`Unix` 分支被 `filter_map` 直接丢弃，**不记录、不告警**。

```16:26:Edgion/src/core/gateway/lb/selection/consistent_hash.rs
    pub fn build(backends: &[Backend]) -> Self {
        let buckets: Vec<_> = backends
            .iter()
            .filter_map(|b| {
                if let SocketAddr::Inet(addr) = b.addr {
                    Some(Bucket::new(addr, b.weight.max(1) as u32))
                } else {
                    None
                }
            })
            .collect();
```

若后端列表**全部为 UDS**，`buckets` 为空；`Continuum::new(&buckets)` 的行为依赖 `pingora_ketama` 实现，存在「空环」或后续 `select` 无法命中合理节点的风险，且与运维对「后端数量」的直觉不符。

### 2. `addr_to_idx` 仅映射 Inet，与环上地址类型一致但无法覆盖 UDS

即使未来通过某种方式让环「感知」虚拟节点，当前映射键类型为 `std::net::SocketAddr`，且构建时同样过滤掉 `Unix`：

```30:40:Edgion/src/core/gateway/lb/selection/consistent_hash.rs
        let addr_to_idx: HashMap<_, _> = backends_vec
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                if let SocketAddr::Inet(addr) = b.addr {
                    Some((addr, i))
                } else {
                    None
                }
            })
            .collect();
```

`select()` 中通过 `self.ring.get_addr(&mut idx)` 取到的地址与 `addr_to_idx` 查找一致的前提是：**环上节点均为 Inet 且在映射表中**；UDS 后端既不在环上也不在表中，**不可能**被该路径选中。

```69:85:Edgion/src/core/gateway/lb/selection/consistent_hash.rs
            let Some(addr) = self.ring.get_addr(&mut idx) else {
                break;
            };

            steps += 1;

            if let Some(&backend_idx) = self.addr_to_idx.get(addr) {
                if seen.contains(&backend_idx) {
                    continue;
                }
                seen.insert(backend_idx);

                let backend = &self.backends[backend_idx];
                if health_filter(backend) {
                    return Some(backend.clone());
                }
            }
```

注意：`backends_vec` 仍保留**完整** `Backend` 列表（含 UDS），与 `buckets` / `addr_to_idx` 子集不一致，加剧「配置与行为不符」的隐蔽性。

### 3. `sp_affinity_build` 与 `sp_affinity_get_or_build` 仅处理 Inet

会话持久化 affinity 以 **CRC32(ip:port 字符串) → `std::net::SocketAddr`** 存储。UDS 没有 `std::net::SocketAddr` 的 Unix 变体（标准库中 Unix 套接字地址在 `std::os::unix::net`），当前实现自然只处理 `Inet`：

```100:108:Edgion/src/core/gateway/lb/runtime_state/mod.rs
pub fn sp_affinity_build(service_key: &str, backends: &[Backend]) {
    let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
    for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
        if let SocketAddr::Inet(addr) = b.addr {
            let addr_str = addr.to_string();
            let token = format!("{:08x}", crc32fast::hash(addr_str.as_bytes()));
            map.insert(token, addr);
        }
    }
```

`sp_affinity_get_or_build` 闭包内逻辑与上**重复**，修复时需**两处同步**，避免一侧仍忽略 UDS。

```121:128:Edgion/src/core/gateway/lb/runtime_state/mod.rs
            let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
            for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
                if let SocketAddr::Inet(addr) = b.addr {
                    let addr_str = addr.to_string();
                    let token = format!("{:08x}", crc32fast::hash(addr_str.as_bytes()));
                    map.insert(token, addr);
                }
            }
```

`sp_affinity_lookup` 返回 `Option<std::net::SocketAddr>`，本身也**无法**表达 UDS 端点，属于 API 层面的连带限制。

### 4. 与 `pingora_ketama` 的关系

当前 `Bucket::new` 使用 **`std::net::SocketAddr`**（来自 Inet 分支）。Rust 标准库中该类型用于 IP 套接字；**ketama 环的数学模型与实现通常按 IP:port 虚拟节点展开**。要为 UDS 提供一致语义，需要：

- 要么 **上游**（如 `pingora_ketama`）扩展为支持基于规范化路径字符串的虚拟节点；
- 要么在本层 **自定义**「逻辑节点 → hash 输入」映射，同时保证 `select` 与 `addr_to_idx` 使用同一套标识。

---

## 修改建议

1. **短期（低成本、高收益）**  
   - 在 `ConsistentHashRing::build` 中：若 `backends` 中存在 `SocketAddr::Unix`，打 **`tracing::warn!`**（可带 `service` 或后端数量统计），明确说明 **CH 当前忽略 UDS**，避免静默失败。  
   - 在 `sp_affinity_build` / `sp_affinity_get_or_build` 中：对 UDS 后端同样 **warn**，并文档化 **SP affinity 仅支持 Inet**。

2. **中期（行为与文档一致）**  
   - 在路由/LB 策略文档或 CRD 说明中写明：**Consistent Hash / Session Persistence 与 UDS 后端组合时行为未定义或仅部分支持**，引导使用 RR 或其他策略。

3. **长期（完整支持 UDS）**  
   - 定义 **稳定的 UDS 规范化字符串**（如绝对路径、NUL 结尾约定等），用于：  
     - 与 CRC32 / hash key 对齐（SP）；  
     - 若引入自定义环或扩展 ketama，用于虚拟节点 hash。  
   - 评估 **`pingora_ketama::Bucket` / `Continuum`** 是否可扩展或是否需 **fork / 替代实现**；若库仅支持 `std::net::SocketAddr`，需 **新类型或并行查找结构** 存储「hash → Backend 索引」，并与现有 `select` 回退逻辑统一。  
   - 扩展 **`sp_affinity_lookup` 返回类型**（例如 `enum { Inet(std::net::SocketAddr), Unix(PathBuf) }` 或统一为 `SocketAddr`），与 Pingora `Backend` 地址模型对齐。

4. **测试**  
   - 增加单元测试：`backends` 混合 Inet + Unix 时 CH 与 SP 的**预期行为**（含 warn 是否触发）；全 UDS 时是否回退、是否返回 `None`，避免回归。

---

## 小结

| 项 | 结论 |
|----|------|
| **根因** | CH 与 SP 路径在数据结构与算法上绑定 **Inet / `std::net::SocketAddr`**，UDS 被 `filter_map` 丢弃且无日志 |
| **风险** | 边缘与开发场景下**负载不均 / 会话错绑**；问题**隐蔽** |
| **建议优先级** | 先 **可观测性（warn）+ 文档**，再视产品需求决定是否完整支持 UDS 上的 ketama 与 SP |
