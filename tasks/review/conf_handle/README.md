# Gateway conf_handler 代码审查报告

> 审查时间: 2026-03-30  
> 审查范围: `src/core/gateway/` 下所有 `conf_handler_impl.rs` 和 `conf_handler.rs` 文件（共 15 个）  
> 基础 trait: `src/core/common/conf_sync/traits.rs` — `ConfHandler<T>`

## 审查覆盖文件

| 模块 | 文件路径 | 行数 | 资源类型 |
|------|---------|------|---------|
| HTTP 路由 | `routes/http/conf_handler_impl.rs` | 652 | HTTPRoute |
| TLS 路由 | `routes/tls/conf_handler_impl.rs` | 500 | TLSRoute |
| TCP 路由 | `routes/tcp/conf_handler_impl.rs` | 276 | TCPRoute |
| gRPC 路由 | `routes/grpc/conf_handler_impl.rs` | 244 | GRPCRoute |
| UDP 路由 | `routes/udp/conf_handler_impl.rs` | 420 | UDPRoute |
| Endpoint | `backends/discovery/endpoint/conf_handler_impl.rs` | 188 | Endpoints |
| EndpointSlice | `backends/discovery/endpoint_slice/conf_handler_impl.rs` | 212 | EndpointSlice |
| Service | `backends/discovery/services/conf_handler_impl.rs` | 181 | Service |
| BackendTLSPolicy | `backends/policy/backend_tls/conf_handler_impl.rs` | 215 | BackendTLSPolicy |
| EdgionGatewayConfig | `config/edgion_gateway/conf_handler_impl.rs` | 127 | EdgionGatewayConfig |
| GatewayClass | `config/gateway_class/conf_handler_impl.rs` | 127 | GatewayClass |
| EdgionPlugins | `plugins/http/conf_handler_impl.rs` | 159 | EdgionPlugins |
| ACME | `services/acme/conf_handler_impl.rs` | 49 | EdgionAcme |
| TLS Store | `tls/store/conf_handler.rs` | 97 | EdgionTls |
| LinkSys | `link_sys/runtime/conf_handler.rs` | 123 | LinkSys |

## 问题总览

共发现 **10 个问题**，按严重程度排列：

### 严重 / 高（需优先修复）

| # | 文档 | 问题 | 严重程度 | 影响 |
|---|------|------|---------|------|
| 01 | [01-vec-store-key-mismatch-bug.md](./01-vec-store-key-mismatch-bug.md) | EdgionGatewayConfig/GatewayClass 使用 Vec 存储，`partial_update` 中 remove 的 key 与 `name_any()` 不匹配，**删除永远不生效** | **严重** | 资源泄漏、跨 namespace 冲突 |
| 02 | [02-tls-partial-update-ordering-bug.md](./02-tls-partial-update-ordering-bug.md) | TLS 路由 `partial_update` 操作顺序为 add→update→remove，与 TCP/UDP 的 remove→update→add 相反 | **高** | 路由状态不一致、listener 脏数据 |

### 中等（应在近期修复）

| # | 文档 | 问题 | 严重程度 | 影响 |
|---|------|------|---------|------|
| 03 | [03-partial-update-ordering-inconsistency.md](./03-partial-update-ordering-inconsistency.md) | 5 种协议路由的 `partial_update` 使用 3 种不同的操作顺序策略 | **中** | 维护负担、copy-paste 错误风险 |
| 04 | [04-cleanup-removed-backends-duplication.md](./04-cleanup-removed-backends-duplication.md) | Endpoint 和 EndpointSlice handler ~80% 代码重复 | **中** | DRY 违反、双倍维护成本 |
| 05 | [05-confhandler-boilerplate-delegation.md](./05-confhandler-boilerplate-delegation.md) | 10 个文件中 ~139 行纯粹的 ConfHandler 委托转发代码 | **中** | 样板代码膨胀 |
| 06 | [06-rwlock-mutex-unwrap-panic-risk.md](./06-rwlock-mutex-unwrap-panic-risk.md) | RwLock/Mutex 使用 `.unwrap()` 在 lock poisoned 时会 panic | **中** | 级联故障风险 |
| 09 | [09-http-route-handler-complexity.md](./09-http-route-handler-complexity.md) | HTTP route handler 652 行，混合了路由解析/编译/验证逻辑 | **中** | 难以维护和扩展 |

### 低（可在后续优化中处理）

| # | 文档 | 问题 | 严重程度 | 影响 |
|---|------|------|---------|------|
| 07 | [07-tls-store-method-name-ambiguity.md](./07-tls-store-method-name-ambiguity.md) | TLS store 的 ConfHandler 实现与固有方法同名，靠参数类型消歧 | **低** | 可读性差、重构风险 |
| 08 | [08-missing-send-sync-bounds.md](./08-missing-send-sync-bounds.md) | 2-3 个 `create_*_handler` 返回类型缺少显式 `Send + Sync` | **低** | 不一致性 |
| 10 | [10-unnecessary-data-clone-performance.md](./10-unnecessary-data-clone-performance.md) | `full_set` 中不必要的 `HashMap` 整体 clone | **低/中** | 大规模同步时内存/CPU 开销 |

## 修复优先级建议

```
P0 (立即修复):
  ├── #01 Vec存储key不匹配 — 删除完全失效是功能性BUG
  └── #02 TLS操作顺序 — 路由状态不一致可导致流量异常

P1 (本迭代):
  ├── #03 统一partial_update顺序 — 与#02一起改
  ├── #06 RwLock unwrap — 防止级联panic
  └── #04 Endpoint/EndpointSlice去重 — 减少维护面

P2 (下一迭代):
  ├── #05 消除委托样板代码 — 宏或blanket impl
  ├── #09 HTTP handler拆分 — 提高可维护性
  └── #10 减少不必要clone — 性能优化

P3 (空闲时):
  ├── #07 TLS store方法名 — 重命名固有方法
  └── #08 Send+Sync一致性 — 统一签名
```

## 架构层面建议

1. **ConfHandler trait 设计改进**: 考虑将 `full_set` 签名改为接收 `HashMap<String, T>` (owned) 以避免不必要克隆
2. **引入编排层**: 在 `ConfHandler` 和具体 handler 之间引入中间层，标准化 partial_update 的操作顺序
3. **使用 `parking_lot` 替代 `std::sync`**: 避免锁中毒问题，同时获得更好的性能
4. **泛型化 Discovery 层**: 用 trait + 泛型参数统一 Endpoint/EndpointSlice handler
