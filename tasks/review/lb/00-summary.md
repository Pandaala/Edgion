# LB（负载均衡）模块代码审查总结

> 审查日期：2026-03-30
> 审查范围：`src/core/gateway/lb/` 及其关联的路由选择、后端发现、预热模块

## 审查概览

本次审查覆盖了 Edgion Gateway 的负载均衡（LB）核心模块，包括：

| 模块 | 路径 | 说明 |
|------|------|------|
| LB 主模块 | `src/core/gateway/lb/` | 选择算法、运行时状态、策略管理 |
| Backend Selector | `src/core/gateway/lb/backend_selector/` | Route 级加权轮询选择 |
| Selection 算法 | `src/core/gateway/lb/selection/` | RR、一致性哈希、最少连接、EWMA |
| LeastConn | `src/core/gateway/lb/leastconn/` | 后端状态、清理器 |
| EWMA | `src/core/gateway/lb/ewma/` | 平滑因子配置 |
| LB Policy | `src/core/gateway/lb/lb_policy/` | 策略存储与配置 |
| Runtime State | `src/core/gateway/lb/runtime_state/` | 全局运行时状态（EWMA/连接计数/缓存） |
| Backends | `src/core/gateway/backends/mod.rs` | 后端选择、peer 构建 |
| Preload | `src/core/gateway/backends/preload.rs` | 启动预热 |
| LB Policy Sync | `src/core/gateway/routes/http/lb_policy_sync.rs` | HTTPRoute 策略同步 |
| Upstream Peer | `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` | 上游 peer 选择 |
| Connected | `src/core/gateway/routes/http/proxy_http/pg_connected_to_upstream.rs` | 连接建立后处理 |
| Logging | `src/core/gateway/routes/http/proxy_http/pg_logging.rs` | 日志阶段 LB 指标更新 |

## 问题汇总

共发现 **12** 个问题，按严重程度分布：

| 严重程度 | 数量 | 说明 |
|---------|------|------|
| **P1** | 1 | 功能异常/内存泄漏 |
| **P2** | 5 | 性能问题/潜在风险 |
| **P3** | 6 | 代码质量/可维护性 |

### P1 - 功能异常/内存泄漏

| # | 问题 | 文件 | 文档 |
|---|------|------|------|
| 01 | Runtime State 全局静态变量的内存泄漏风险 | `lb/runtime_state/mod.rs` | [01-runtime-state-memory-leak.md](./01-runtime-state-memory-leak.md) |

### P2 - 性能问题/潜在风险

| # | 问题 | 文件 | 文档 |
|---|------|------|------|
| 02 | BackendSelector 的 ArcSwap 初始化竞态条件 | `lb/backend_selector/selector.rs` | [02-backend-selector-race-condition.md](./02-backend-selector-race-condition.md) |
| 03 | RoundRobinSelector 不支持平滑加权轮询 | `lb/selection/round_robin.rs` | [03-round-robin-smoothness-inconsistency.md](./03-round-robin-smoothness-inconsistency.md) |
| 05 | EWMA 整数运算精度损失 | `lb/runtime_state/mod.rs` | [05-ewma-integer-precision-loss.md](./05-ewma-integer-precision-loss.md) |
| 06 | Session Persistence Affinity Map 的 CRC32 碰撞与一致性问题 | `lb/runtime_state/mod.rs` | [06-session-persistence-crc32-collision.md](./06-session-persistence-crc32-collision.md) |
| 08 | BackendCleaner 停止不等待任务完成且缺少优雅关闭 | `lb/leastconn/cleaner.rs` | [08-backend-cleaner-graceful-shutdown.md](./08-backend-cleaner-graceful-shutdown.md) |

### P3 - 代码质量/可维护性

| # | 问题 | 文件 | 文档 |
|---|------|------|------|
| 04 | ConsistentHashRing 不支持 Unix Domain Socket | `lb/selection/consistent_hash.rs` | [04-consistent-hash-unix-socket-gap.md](./04-consistent-hash-unix-socket-gap.md) |
| 07 | PolicyStore 的 remove_by_route 中不必要的 clone 开销 | `lb/lb_policy/policy_store.rs` | [07-policy-store-unnecessary-clone.md](./07-policy-store-unnecessary-clone.md) |
| 09 | select_by_min_metric 的 BinaryHeap 小规模后端性能问题 | `lb/selection/mod.rs` | [09-select-by-min-metric-performance.md](./09-select-by-min-metric-performance.md) |
| 10 | try_get_peer 中大量重复代码 | `backends/mod.rs` | [10-try-get-peer-code-duplication.md](./10-try-get-peer-code-duplication.md) |
| 11 | select_from_backends 参数过多（9个参数） | `backends/mod.rs` | [11-select-from-backends-too-many-params.md](./11-select-from-backends-too-many-params.md) |
| 12 | Preload 模块 lb_type 参数未实际使用 | `backends/preload.rs` | [12-preload-unused-lb-type-param.md](./12-preload-unused-lb-type-param.md) |

## 架构分析

### LB 模块架构总览

```
请求进入
  │
  ├─ Route 匹配 → BackendSelector (WeightedRoundRobin) → 选择 Service (backend_ref)
  │                                                         │
  │                                                         ▼
  ├─ 后端选择 → select_backend_by_policy()
  │              │
  │              ├─ 无 LB Policy → RoundRobinSelector (RR_CACHE)
  │              ├─ ConsistentHash → ConsistentHashRing (CH_CACHE) + hash key 提取
  │              ├─ LeastConn → select_by_min_metric(conn_counts)
  │              └─ EWMA → select_by_min_metric(ewma_values)
  │
  ├─ 连接建立 (connected_to_upstream) → LeastConn: increment()
  │
  └─ 日志阶段 (logging) → LeastConn: decrement() / EWMA: update_ewma()
```

### 两层加权轮询的设计

| 层级 | 实现 | 算法 | 用途 |
|------|------|------|------|
| Route 层 | `BackendSelector<T>` → `WeightedRoundRobin<T>` | Smooth WRR | 在多个 Service 间按权重分配流量 |
| Endpoint 层 | `RoundRobinSelector` (runtime_state 缓存) | 简单加权重复 | 在同一 Service 的 Endpoints 间分配 |

**问题**：两层使用了不同的 WRR 算法，Endpoint 层缺少平滑分布（详见 #03）。

### 运行时状态管理

全局静态 `DashMap` 管理所有 service 的 LB 运行时状态：

- `SERVICES` → 每个 service 的 EWMA、连接计数、后端状态
- `RR_CACHE` → 每个 service 的 RoundRobinSelector 缓存
- `CH_CACHE` → 每个 service 的 ConsistentHashRing 缓存
- `SP_AFFINITY` → 每个 service 的 Session Persistence 亲和映射

**问题**：缺少 TTL 和主动过期机制，长期运行可能累积过时状态（详见 #01）。

## 优先修改建议

### 第一批（P1 + 高影响 P2）

1. **#01 Runtime State 内存泄漏**：添加定期清理机制和 TTL
2. **#03 RoundRobinSelector 平滑化**：统一使用 Smooth WRR
3. **#08 BackendCleaner 优雅关闭**：保存 JoinHandle，支持 await 退出

### 第二批（P2）

4. **#05 EWMA 精度**：使用更高精度的定点数
5. **#06 Session Persistence**：升级 hash 函数，审视 token 设计
6. **#02 BackendSelector 竞态**：使用 CAS 或 OnceLock

### 第三批（P3 代码质量）

7. **#10 try_get_peer 重复代码**：提取公共函数
8. **#11 参数过多**：引入 BackendSource 枚举
9. **#07 PolicyStore clone**：直接修改引用
10. **#09 BinaryHeap 效率**：小规模线性扫描
11. **#04 ConsistentHash UDS**：添加 warn 日志
12. **#12 Preload lb_type**：简化或实际利用

## 严重程度定义

| 等级 | 说明 | 示例 |
|------|------|------|
| **P0** | 数据丢失/服务崩溃 | panic、数据损坏 |
| **P1** | 功能异常/内存泄漏 | 状态不清理、逻辑错误 |
| **P2** | 性能问题/潜在风险 | 精度损失、竞态条件 |
| **P3** | 代码质量/可维护性 | 重复代码、参数过多 |
