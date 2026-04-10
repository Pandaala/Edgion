# Gateway Routes & Services — 锁中毒 Review

## 模块概述

本次 review 覆盖 Gateway 数据面的路由管理、ACME 证书挑战存储、JWT 认证插件三个模块中的所有锁使用点。

关键架构特征：
- **HTTP/gRPC 路由热路径** 使用 `ArcSwap` 实现无锁读取，`RwLock` 仅在路由重建（config sync 回调）和 `Clone` impl 中使用
- **ACME challenge store** 的 `RwLock` 在每个 HTTP 请求的 `early_request_filter` 中被 read，但有 `is_empty()` 快速路径（99.99% 时间无活跃挑战直接返回）
- **JWT auth** 的 `RwLock` 在首次请求时 write 一次（lazy load），后续请求 read
- **UDP listener registry** 使用 `std::sync::RwLock`，仅在监听器注册/注销时 write

## 锁使用清单

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|------|------|------|--------|----------|----------|
| 1 | `routes/grpc/conf_handler_impl.rs` | 28 | `route_units_cache.lock().unwrap()` | `std::sync::Mutex` | `unwrap()` | 低 |
| 2 | `routes/grpc/conf_handler_impl.rs` | 81 | `route_units_cache.lock().unwrap()` | `std::sync::Mutex` | `unwrap()` | 低 |
| 3 | `routes/grpc/routes_mgr.rs` | 101 | `resource_keys.read().unwrap()` (Clone impl) | `std::sync::RwLock` | `unwrap()` | 低 |
| 4 | `routes/grpc/routes_mgr.rs` | 102 | `route_rules_list.read().unwrap()` (Clone impl) | `std::sync::RwLock` | `unwrap()` | 低 |
| 5 | `routes/grpc/routes_mgr.rs` | 267 | `route_units_cache.lock().unwrap()` | `std::sync::Mutex` | `unwrap()` | 低 |
| 6 | `routes/grpc/routes_mgr.rs` | 311 | `route_units_cache.lock().unwrap()` | `std::sync::Mutex` | `unwrap()` | 低 |
| 7 | `routes/grpc/routes_mgr.rs` | 341 | `route_units_cache.lock().unwrap()` | `std::sync::Mutex` | `unwrap()` | 低 |
| 8 | `routes/http/routes_mgr.rs` | 43 | `resource_keys.read().unwrap()` (Clone impl) | `std::sync::RwLock` | `unwrap()` | 低 |
| 9 | `routes/http/routes_mgr.rs` | 44 | `route_rules_list.read().unwrap()` (Clone impl) | `std::sync::RwLock` | `unwrap()` | 低 |
| 10 | `routes/http/routes_mgr.rs` | 46 | `regex_routes.read().unwrap()` (Clone impl) | `std::sync::RwLock` | `unwrap()` | 低 |
| 11 | `routes/udp/mod.rs` | 45 | `tokens.write().expect(...)` | `std::sync::RwLock` | `expect()` | 低 |
| 12 | `routes/udp/mod.rs` | 54 | `tokens.write().expect(...)` | `std::sync::RwLock` | `expect()` | 低 |
| 13 | `routes/udp/mod.rs` | 65 | `tokens.write().expect(...)` | `std::sync::RwLock` | `expect()` | 低 |
| 14 | `services/acme/challenge_store.rs` | 50 | `tokens.read().unwrap()` (`is_empty`) | `std::sync::RwLock` | `unwrap()` | 中 |
| 15 | `services/acme/challenge_store.rs` | 56 | `tokens.read().unwrap()` (`lookup`) | `std::sync::RwLock` | `unwrap()` | 中 |
| 16 | `services/acme/challenge_store.rs` | 90 | `tokens.write().unwrap()` (`full_set`) | `std::sync::RwLock` | `unwrap()` | 低 |
| 17 | `services/acme/challenge_store.rs` | 91 | `resource_tokens.write().unwrap()` (`full_set`) | `std::sync::RwLock` | `unwrap()` | 低 |
| 18 | `services/acme/challenge_store.rs` | 125 | `tokens.write().unwrap()` (`partial_update`) | `std::sync::RwLock` | `unwrap()` | 低 |
| 19 | `services/acme/challenge_store.rs` | 126 | `resource_tokens.write().unwrap()` (`partial_update`) | `std::sync::RwLock` | `unwrap()` | 低 |
| 20 | `plugins/http/jwt_auth/plugin.rs` | 136-137 | `credentials.read().map_err(...)` | `std::sync::RwLock` | `map_err` (正确处理) | 低 |
| 21 | `plugins/http/jwt_auth/plugin.rs` | 149-151 | `credentials.write().map_err(...)` | `std::sync::RwLock` | `map_err` (正确处理) | 低 |

**注**：`routes/udp/edgion_udp.rs` 中的 `last_activity.lock()` (行 208, 285, 316) 使用的是 `parking_lot::Mutex`，不会中毒，不在此表中。测试代码中的锁使用 (tests.rs, plugin.rs tests) 也已排除。

## 问题分析

### 1. ACME Challenge Store — 每个 HTTP 请求都会 read（#14-15）

**路径**：`early_request_filter` → `get_global_challenge_store().is_empty()` → `tokens.read().unwrap()`

这是本次 review 中唯一真正在请求热路径上的 `std::sync::RwLock` read。每个 HTTP 请求都会执行 `is_empty()` 检查。

**中毒场景分析**：
- `is_empty()` 只取 read lock，不会 panic，因此不会毒化锁
- `lookup()` 也只取 read lock，同样不会 panic
- `full_set()` 和 `partial_update()` 取 write lock，内部操作是纯 HashMap 操作，不会 panic
- **结论**：中毒风险极低。HashMap 的 insert/remove/clear 不会 panic（除非 OOM，此时整个进程已经崩溃）

**但若确实中毒了**（理论上）：
- **爆炸半径**：**全量 HTTP 流量**。每个请求都会在 `early_request_filter` 中 panic，导致该请求被 Pingora 中止
- 不仅 ACME 挑战失败，所有 HTTP 代理都会停止工作
- 这是目前中毒后果最严重的点

### 2. gRPC route_units_cache Mutex（#1-2, #5-7）

`GlobalGrpcRouteManagers.route_units_cache` 是 `std::sync::Mutex`，在以下场景获取锁：
- `full_set`（config sync 回调）— write
- `partial_update`（config sync 回调）— write
- `rebuild_affected_listener_managers` — read-like（lock 后只读取）
- `bucket_units_by_listener` — read-like
- `stats` — read-like

**中毒场景分析**：
- 所有使用点都在 config sync 的单线程回调中，且操作为纯 HashMap 操作
- 不在请求热路径上（请求通过 `ArcSwap` 读取已构建好的路由快照）
- **若中毒**：gRPC 路由更新停止，新的路由配置无法生效，但已有路由继续工作

### 3. RouteRules / GrpcRouteRules 的 Clone impl（#3-4, #8-10）

这些 `read().unwrap()` 出现在 `Clone` trait 实现中，用于将 RwLock 内的数据 clone 到新的 RwLock 中。

**关键发现**：经过代码追踪，这些 Clone 实际上并未在任何地方被调用：
- `RouteRules` 和 `GrpcRouteRules` 在 `build_*_from_routes/units` 函数中通过 `RouteRules { ... }` 直接构造，使用 `RwLock::new()`
- `DomainRouteRules` 和 `DomainGrpcRouteRules` 也是直接构造
- 这些 `Clone` impl 看起来是遗留代码，`RwLock` 字段在运行时从未被 read/write

**若中毒**：无实际影响，因为这些 RwLock 在运行时不被访问。但 Clone impl 的存在暗示曾经有热路径读取，应视为代码异味。

### 4. UDP Listener Registry（#11-13）

`UdpListenerRegistry.tokens` 使用 `std::sync::RwLock`，仅在监听器注册/注销/关闭时 write。

**中毒场景分析**：
- 仅在 config sync 回调和 graceful shutdown 时使用
- 不在请求热路径上
- **若中毒**：UDP 监听器无法注册/注销/关闭。影响范围有限，仅影响 UDP 路由的热更新

### 5. JWT Auth credentials（#20-21）—— 已正确处理

`JwtAuth.credentials` 的锁获取使用了 `map_err` 而非 `unwrap`，将中毒错误转换为认证错误返回给客户端（401），不会 panic。

**这是本次 review 中唯一正确处理锁中毒的代码**，可以作为其他模块的参考模式。

## 最佳处理方案

### 方案 A：ACME Challenge Store — 迁移到 `arc_swap`（推荐）

ACME store 是唯一在每请求热路径上的 `std::sync::RwLock`。推荐迁移到与路由管理器相同的 `ArcSwap` 模式：

```rust
pub struct AcmeChallengeStore {
    // 替换 RwLock<HashMap> 为 ArcSwap<HashMap>
    tokens: ArcSwap<HashMap<String, ChallengeEntry>>,
    // resource_tokens 仅在 config sync 中使用，可保留 RwLock 或用 Mutex
    resource_tokens: Mutex<HashMap<String, Vec<String>>>,
}
```

好处：
- `is_empty()` 和 `lookup()` 变为完全无锁
- 消除中毒风险
- 与项目现有的 `ArcSwap` 路由模式一致

### 方案 B：gRPC route_units_cache — 迁移到 `parking_lot::Mutex`（推荐）

`parking_lot::Mutex` 不会中毒，项目已有依赖（`parking_lot = "0.12"`）。简单替换即可：

```rust
// routes_mgr.rs
pub(crate) route_units_cache: parking_lot::Mutex<HashMap<String, Vec<Arc<GrpcRouteRuleUnit>>>>,
```

### 方案 C：RouteRules / GrpcRouteRules 的 RwLock 字段 — 移除或冻结

这些 RwLock 字段 (`resource_keys`, `route_rules_list`, `regex_routes`) 在运行时不被访问。两个选择：
1. **移除 RwLock 包装**：如果确认没有运行时写入，直接改为裸字段
2. **保持现状**：风险已经为零（未被调用），低优先级

### 方案 D：UDP Listener Registry — 迁移到 `parking_lot::RwLock`

简单替换，消除中毒可能：

```rust
use parking_lot::RwLock;  // 替换 std::sync::RwLock
```

### 方案 E：JWT Auth — 无需修改

已使用 `map_err` 正确处理中毒，是项目中的最佳实践范例。

## 优先级

| 优先级 | 项目 | 理由 |
|--------|------|------|
| P1 | ACME Challenge Store 迁移到 ArcSwap (#14-15) | 每请求热路径上唯一的 std::sync::RwLock；中毒会瘫痪全量 HTTP 流量；且迁移到 ArcSwap 可消除不必要的锁竞争 |
| P2 | gRPC route_units_cache 迁移到 parking_lot::Mutex (#1-2, #5-7) | config sync 路径，中毒导致路由更新停止；简单替换 |
| P3 | UDP Listener Registry 迁移到 parking_lot::RwLock (#11-13) | 低频使用，中毒仅影响 UDP 热更新；简单替换 |
| P4 | RouteRules/GrpcRouteRules Clone impl 中的 RwLock read (#3-4, #8-10) | 运行时未被调用，实际风险为零；建议评估是否可以移除 RwLock 包装简化代码 |
| -- | JWT Auth credentials (#20-21) | 已正确处理，无需修改 |

**总结**：本批 21 个锁获取点中，仅 ACME store 的 2 个 read 在请求热路径上 (#14-15)，中毒爆炸半径为全量 HTTP 流量，应优先修复。其余均在 config sync 或管理路径上，实际中毒概率极低但应逐步迁移到 `parking_lot` 以消除隐患。JWT auth 是唯一正确处理中毒的模块，值得推广其模式。
