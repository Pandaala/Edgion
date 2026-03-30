# [P2] Session Persistence Affinity Map 的 CRC32 碰撞与一致性问题

## 问题描述

会话保持（Session Persistence）依赖「客户端携带的 token」在网关侧解析为固定的上游 `SocketAddr`。当前实现将每个后端的 `ip:port` 字符串经 **CRC32** 哈希后格式化为 **8 位十六进制**（`{:08x}`）作为 map 的键，并与 `session_persistence.rs` 中 `generate_session_token` 的算法保持一致，使 Set-Cookie 与 affinity map 可互查。

该设计带来多层风险：

1. **CRC32 空间有限（2³²）**：多后端时存在不可忽视的 **生日悖论碰撞**；`HashMap::insert` 在键冲突时 **静默覆盖**，导致部分后端从 affinity map 中消失，对应会话 token 永远无法命中真实后端。
2. **语义耦合**：token 本质是「后端地址的摘要」，而非独立于拓扑的会话标识；地址变更、NAT、双栈表示差异（如 `127.0.0.1` vs `localhost`）都会改变 token 与运维直觉。
3. **缓存与后端列表一致性**：`sp_affinity_get_or_build` 采用 `or_insert_with` **仅在首次** 根据当时的 `backends` 构建 `Arc<HashMap>`；后续 endpoint 变化依赖 `invalidate_selector_cache` 清除 `SP_AFFINITY`。若失效调用缺失、延迟或路径不一致，会出现 **map 指向已下线地址** 或 **未包含新后端** 的窗口。
4. **不可变快照与重建成本**：`Arc<HashMap>` 整体替换是合理并发模型，但在后端高频抖动时与其他缓存（RR/CH）一同反复全量构建，存在 **CPU 与分配压力**（与 P2 性能类问题相邻）。
5. **容量硬编码**：`SP_AFFINITY_MAX_PER_SERVICE = 1024` 无配置入口，超大 Service 下 **截断** 与 **可观测性**（哪部分后端从未进入 map）不足。
6. **API 冗余**：`sp_affinity_build` 已实现但 **仓库内无任何调用方**，实际路径仅 `sp_affinity_get_or_build`；易造成「已主动刷新」的误读。

## 影响范围

| 类别 | 说明 |
|------|------|
| **Affinity 存储与构建** | [`runtime_state/mod.rs`](../../../src/core/gateway/lb/runtime_state/mod.rs)：`SP_AFFINITY`、`sp_affinity_lookup`、`sp_affinity_build`、`sp_affinity_get_or_build`、`invalidate_selector_cache`。 |
| **Token 生成与解析** | [`session_persistence.rs`](../../../src/core/gateway/routes/http/session_persistence.rs)：`generate_session_token`（同为 CRC32 + `{:08x}`）。 |
| **请求路径** | [`pg_upstream_peer.rs`](../../../src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs)：在 session persistence 激活且请求已带 token 时，`sp_affinity_get_or_build` + `affinity_map.get(token)`，未命中则回退普通 LB 并注入新 token。 |
| **缓存失效** | [`endpoint/conf_handler_impl.rs`](../../../src/core/gateway/backends/discovery/endpoint/conf_handler_impl.rs)、[`endpoint_slice/conf_handler_impl.rs`](../../../src/core/gateway/backends/discovery/endpoint_slice/conf_handler_impl.rs)：在 endpoint 变更时调用 `invalidate_selector_cache`（含 `SP_AFFINITY.remove`）。 |

## 严重程度

**P2（中）**

- **正确性**：CRC32 碰撞在「单 Service 后端数较小」时概率低，但无检测与告警；一旦碰撞，行为为 **确定性错误**（某后端永不被会话保持选中），属于数据面逻辑缺陷而非崩溃类问题。
- **可用性/体验**：失效路径依赖发现层；异常时用户可能频繁 **丢失粘性**（token 不在 map）或 **粘到幽灵地址**（若下游已不可用且上游仍按 token 定向）。
- **安全**：token 可枚举空间为 2³²，在可预测后端集合场景下存在 **离线猜测 cookie 值** 的理论面（通常仍受业务认证约束；是否构成威胁需结合威胁模型）。

## 代码位置

### 核心实现（CRC32 建表与惰性构建）

```51:133:Edgion/src/core/gateway/lb/runtime_state/mod.rs
/// Session affinity map: token (CRC32 hex) → endpoint SocketAddr, per service key.
/// Built lazily on first session persistence request. Invalidated when backends change.
/// Cap: max 1024 entries per service to prevent memory explosion.
static SP_AFFINITY: LazyLock<DashMap<String, Arc<std::collections::HashMap<String, std::net::SocketAddr>>>> =
    LazyLock::new(DashMap::new);

/// Max entries per service in the session affinity map.
const SP_AFFINITY_MAX_PER_SERVICE: usize = 1024;
// ...
pub fn invalidate_selector_cache(service_key: &str) {
    RR_CACHE.remove(service_key);
    CH_CACHE.remove(service_key);
    SP_AFFINITY.remove(service_key);
}
// ...
pub fn sp_affinity_build(service_key: &str, backends: &[Backend]) {
    let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
    for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
        if let SocketAddr::Inet(addr) = b.addr {
            let addr_str = addr.to_string();
            let token = format!("{:08x}", crc32fast::hash(addr_str.as_bytes()));
            map.insert(token, addr);
        }
    }
    SP_AFFINITY.insert(service_key.to_string(), Arc::new(map));
}

pub fn sp_affinity_get_or_build(
    service_key: &str,
    backends: &[Backend],
) -> Arc<std::collections::HashMap<String, std::net::SocketAddr>> {
    SP_AFFINITY
        .entry(service_key.to_string())
        .or_insert_with(|| {
            let mut map = std::collections::HashMap::with_capacity(backends.len().min(SP_AFFINITY_MAX_PER_SERVICE));
            for b in backends.iter().take(SP_AFFINITY_MAX_PER_SERVICE) {
                if let SocketAddr::Inet(addr) = b.addr {
                    let addr_str = addr.to_string();
                    let token = format!("{:08x}", crc32fast::hash(addr_str.as_bytes()));
                    map.insert(token, addr);
                }
            }
            Arc::new(map)
        })
        .value()
        .clone()
}
```

### Token 生成（须与 map 键算法同步演进）

```12:18:Edgion/src/core/gateway/routes/http/session_persistence.rs
/// Generate a session token from a backend address (ip:port).
///
/// Returns a deterministic 8-character lowercase hex string (CRC32 hash).
pub fn generate_session_token(backend_addr: &str) -> String {
    let hash = crc32fast::hash(backend_addr.as_bytes());
    format!("{:08x}", hash)
}
```

### 请求路径上的查表

```356:377:Edgion/src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs
    if sp_active {
        if let Some(ref token) = sp_token_value {
            // ...
            let backends = crate::core::gateway::backends::get_backends_for_service(&service_key);
            let affinity_map =
                crate::core::gateway::lb::runtime_state::sp_affinity_get_or_build(&service_key, &backends);

            if let Some(&addr) = affinity_map.get(token.as_str()) {
                ctx.sp_target_addr = Some(addr);
            } else {
                ctx.sp_state = SessionPersistenceState::InjectNew;
            }
        }
    }
```

## 碰撞概率分析

将每个后端地址映射到 **32 位** CRC32 输出空间（共 \(M = 2^{32}\) 个槽位）。设某 Service 在 map 中实际纳入 **\(n\)** 个互不相同的后端地址（且已忽略 1024 截断时 \(n \leq 1024\) 的更强约束），在 **理想随机哈希** 近似下，至少发生一次碰撞的概率为：

\[
P(\text{collision}) \approx 1 - \exp\left(-\frac{n(n-1)}{2M}\right)
\]

- **\(n = 1024\)**（达到当前上限）：\(P \approx 1 - e^{-1024 \times 1023 / (2 \times 2^{32})} \approx 1.2 \times 10^{-4}\)（约 **0.01%** 量级），单 Service 仍非零。
- **生日悖论尺度**：若在同一逻辑空间内独立采样约 **\( \sqrt{2M\ln 2} \approx 7.7 \times 10^4 \)** 个值，「至少一对碰撞」概率约 **50%**。单 Service 极少达到该规模，但 **多 Service 全集群** 或 **长期累计曾出现过的地址集合** 会抬高「见过一次碰撞」的期望。

**实现层面的额外注意**：

- CRC32 **并非密码学哈希**，且对输入为 `Display` 格式的 `ip:port` 字符串；不同字符串表示同一套接字时可能产生 **人为不一致**（虽同一 `SocketAddr::to_string()` 路径下通常稳定）。
- `HashMap` 在 **键冲突** 时后写覆盖先写，**无日志、无指标**，运维上难以发现「少了一个后端」。

## 详细分析补充

### Token 方向与产品语义

当前数据流为：`crc32("ip:port")` → 写入 Cookie/Header；请求带回该值 → 在 `SP_AFFINITY` 中反查 `SocketAddr`。这在工程上实现了「无状态网关 + 确定性粘性」，但 **cookie 值泄露的是后端拓扑的压缩指纹**，且与 **Kubernetes Endpoint 频繁换 IP** 的自然模型相悖：更理想的产品语义往往是 **opaque session id → 由网关或侧车维护的粘性表**，或 **一致性哈希环上的虚拟节点**，而非直接暴露地址摘要。

### `sp_affinity_get_or_build` 与 `backends` 参数

`or_insert_with` 闭包 **不会** 使用调用方传入的 `backends`，仅在 **首次插入** 时构建；首次并发下由赢得 `DashMap::entry` 的线程决定使用哪一快照。之后即使 `get_backends_for_service` 返回新列表，只要 `SP_AFFINITY` 未被 `invalidate_selector_cache` 移除，**内存中仍为旧 map**。这与 RR/CH 缓存的失效模型一致，但将 **正确性完全绑定在发现路径是否每次变更都失效缓存** 上。

### `sp_affinity_build` 未使用

`sp_affinity_build` 与 `get_or_build` 内联了相同建表逻辑，但 **全仓库无 `sp_affinity_build` 调用**。若未来希望在 endpoint 更新时 **同步重建** affinity（与 `invalidate` + 下次懒建相比），应统一入口并避免双份实现漂移。

## 修改建议

1. **哈希函数与键空间**  
   - 将会话 token 与 map 键的生成改为 **64 位或更长** 的非密码学高速哈希（如 **xxHash64**），或 **SipHash**（若需缓解跨租户猜测）；十六进制长度与 cookie 规范（大小、字符集）一并评估。  
   - 迁移期需 **版本化 cookie** 或双读（旧 CRC32 / 新 hash），避免线上已发 cookie 全部失效。

2. **Token 语义解耦**  
   - 评估将 token 改为 **随机 opaque id**（服务端签发）或 **网关内部 id → addr** 的独立表（可带 TTL），使 cookie **不直接编码后端地址的摘要**；后端变更时可通过 **重绑定** 或 **一致性哈希** 减少全表失效。

3. **一致性与失效**  
   - 为 affinity 快照增加 **配置世代号 / endpoints 资源版本**，在 `get_or_build` 时若版本不匹配则 **强制重建**，降低「仅依赖外部 `invalidate`」的耦合。  
   - 对 `invalidate_selector_cache` 的调用链做 **审计与集成测试**（endpoint 全量/增量、slice、竞态）。

4. **碰撞与截断可观测**  
   - 建表时检测 `insert` 是否覆盖已有键（或预先统计 CRC32 冲突），**计数器 + 日志**；超过 `SP_AFFINITY_MAX_PER_SERVICE` 时 **metric + 文档化** 截断策略。

5. **配置化上限**  
   - 将 `SP_AFFINITY_MAX_PER_SERVICE` 纳入 **网关或数据面配置**，并设合理默认与硬上限，防止误配导致内存失控。

6. **代码卫生**  
   - 合并 `sp_affinity_build` 与 `get_or_build` 的建表逻辑为 **单一函数**；若保留显式 `build`，在 endpoint 更新回调中 **明确调用** 或与 `invalidate` 语义写清二选一，避免死代码与误读。

---

**结论**：当前实现在中小规模下可工作，但 **CRC32 + 静默覆盖 + 强依赖失效路径 + 地址摘要式 token** 构成 P2 级技术与产品债务；建议在演进会话保持方案时 **同步** 调整 `runtime_state` 与 `session_persistence` 的算法与失效模型，并补齐观测与配置。
