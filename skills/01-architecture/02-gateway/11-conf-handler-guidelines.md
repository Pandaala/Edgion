# ConfHandler 开发规范

> Gateway 侧 ConfHandler<T> 的职责、分类、增量更新约束和安全要求。

## 1. 什么是 ConfHandler

`ConfHandler<T>` 是 **Gateway 数据面**接收并处理配置变更的标准接口（定义于 `src/core/common/conf_sync/traits.rs`）。

```
Controller → gRPC Watch/List → ClientCache → ConfHandler<T>.full_set / partial_update
```

- `full_set(&HashMap<String, T>)` — 全量替换，由 `List()` 或 gRPC 重连后触发
- `partial_update(add, update, remove)` — 增量更新，由 Watch 事件经 100ms 压缩窗口后触发

每种 K8s/CRD 资源在 `ConfigClient::new()` 中注册一个 `ConfHandler`。

## 2. 资源分类

| 分类 | 特征 | 代表资源 | 更新策略 |
|------|------|---------|---------|
| **存储型** | 数据存入 KV store，请求时按 key 查找 | Service、EdgionPlugins、EdgionTls、BackendTLSPolicy、LinkSys、EdgionAcme、EdgionStreamPlugins | insert/remove 即可 |
| **引擎型** | 需构建匹配引擎（radix tree、regex set 等） | HTTPRoute、GRPCRoute、TLSRoute、TCPRoute、UDPRoute | 必须实现影响范围计算 |
| **配置型** | 低频、少量的基础配置 | Gateway、GatewayClass、EdgionGatewayConfig | 可容忍全量重建 |

## 3. 增量更新规范

### 3.1 禁止"假增量"

`partial_update` 不得在内部 merge 到全量数据后调用等同于 `full_set` 的全量 build。
如果暂时无法做真正增量，**必须**标注 `// TODO: incremental rebuild`。

### 3.2 引擎型资源必须计算影响范围

```
partial_update(add, update, remove)
  → 计算 affected_scope (hostname / port / service_key)
  → 只对受影响的子引擎 rebuild
  → 未受影响部分通过 Arc 复用
```

参考实现：HTTPRoute 的 `build_affected_hostnames` + per-hostname `rebuild_exact_hostname`。

### 3.3 ArcSwap 仅用于数据面

- 数据面（Gateway）的热路径 store 使用 `ArcSwap`，实现 lock-free 读取
- 控制面（Controller）不使用 `ArcSwap`，用 `RwLock` 即可
- 写端模式：clone current map → modify → `store(Arc::new(new_map))`

## 4. 配置泄漏防护

- ConfHandler **不得**在日志中打印资源的完整内容（Secret、TLS 证书等）
- 日志只记录 `key_name`、计数、affected scope 等元数据
- 错误日志只包含错误信息，不含资源 spec
- 参考 [01-log-safety.md](../03-coding/01-log-safety.md) 的完整规范

## 5. 注意事项

- `full_set` 和 `partial_update` 在同一线程（压缩事件处理线程）执行，不会并发调用
- `full_set` 在 gRPC 重连/relist 时触发，必须能处理空 data（清空状态）
- `partial_update` 的 add/update 已经过 preparse（Controller 侧），但 Gateway 侧可能需要额外 preparse（如 GRPCRoute、EdgionPlugins）
- 路由表更新必须是原子的（ArcSwap::store），不能出现"半构建"的中间态

## 6. 现有 Handler 清单

| Handler | 资源 | 文件路径 |
|---------|------|---------|
| RouteManager | HTTPRoute | `routes/http/conf_handler_impl.rs` |
| GrpcRouteManager | GRPCRoute | `routes/grpc/conf_handler_impl.rs` |
| GlobalTlsRouteManagers | TLSRoute | `routes/tls/conf_handler_impl.rs` |
| GlobalTcpRouteManagers | TCPRoute | `routes/tcp/conf_handler_impl.rs` |
| GlobalUdpRouteManagers | UDPRoute | `routes/udp/conf_handler_impl.rs` |
| PluginStore | EdgionPlugins | `plugins/http/conf_handler_impl.rs` |
| StreamPluginStore | EdgionStreamPlugins | `plugins/stream/stream_plugin_store.rs` |
| TlsStore | EdgionTls | `tls/store/conf_handler.rs` |
| ServiceStore | Service | `backends/discovery/services/conf_handler_impl.rs` |
| EpSliceHandler | EndpointSlice | `backends/discovery/endpoint_slice/conf_handler_impl.rs` |
| EndpointHandler | Endpoints | `backends/discovery/endpoint/conf_handler_impl.rs` |
| BackendTLSPolicyStore | BackendTLSPolicy | `backends/policy/backend_tls/conf_handler_impl.rs` |
| GatewayHandler | Gateway | `runtime/handler.rs` |
| GatewayClassHandler | GatewayClass | `config/gateway_class/conf_handler_impl.rs` |
| EdgionGatewayConfigHandler | EdgionGatewayConfig | `config/edgion_gateway/conf_handler_impl.rs` |
| AcmeConfHandler | EdgionAcme | `services/acme/conf_handler_impl.rs` |
| LinkSysStore | LinkSys | `link_sys/runtime/conf_handler.rs` |

> 所有路径相对于 `src/core/gateway/`。

## 7. 引擎型资源增量更新最佳实践

引擎型资源（需构建匹配引擎的）的 `partial_update` 应遵循以下模式：

### 7.1 按影响范围 rebuild（推荐）

适用于有多维度分桶的资源（HTTPRoute 按 hostname，TLSRoute 按 port）。

```
partial_update:
  1. 在修改 cache 之前，收集旧数据的 affected_scope（旧 hostname / 旧 port）
  2. 修改 cache（insert / remove）
  3. 收集新数据的 affected_scope（新 hostname / 新 port）
  4. 合并 affected_scope = old ∪ new
  5. 只对 affected_scope 内的子引擎 rebuild
```

参考：
- HTTPRoute `build_affected_hostnames` → per-hostname `rebuild_exact_hostname`
- TLSRoute `rebuild_affected_port_managers(&affected_ports)`

### 7.2 缓存解析结果（次优）

适用于无分桶维度的全局引擎（GRPCRoute）。

```
partial_update:
  1. 只对 add/update 的 route 做 parse → 更新 route_units_cache
  2. 删除 remove 的 cache entry
  3. 合并 cache 全部 units → 重建引擎
```

省去了对未变更 route 的 preparse + 校验 + unit 构建，也避免了全量 HashMap 深拷贝。

### 7.3 短路判断（兜底）

适用于数量极少但有昂贵重建操作的资源（Gateway）。

```
partial_update:
  if add.is_empty() && update.is_empty() && remove.is_empty() { return; }
  // ... 执行变更和重建
```

## 8. 各资源增量能力评级

| 资源 | 评级 | 策略 |
|------|------|------|
| HTTPRoute | A | 按 affected hostname 精确 rebuild，未变 hostname 通过 Arc 复用 |
| TLSRoute | A | 按 affected port rebuild，未变 port 不触发 |
| TCPRoute | A | 逐条 add_route / remove_route，无全量 rebuild |
| UDPRoute | A | 同 TCPRoute |
| EndpointSlice | A | 按 affected_services 增量更新 LB |
| Endpoints | A | 同 EndpointSlice |
| GRPCRoute | B | route_units_cache 增量，但仍需合并全部 units 重建 GrpcMatchEngine |
| Service | B | Store 增量 + 逐条 health check reconcile |
| EdgionPlugins | B | ArcSwap clone-modify-swap |
| EdgionTls | B | RwLock 逐条 insert/remove |
| EdgionStreamPlugins | B | ArcSwap clone-modify-swap |
| BackendTLSPolicy | B | ArcSwap clone-modify-swap |
| LinkSys | B | ArcSwap clone-modify-swap |
| EdgionAcme | B | challenge store 增量 |
| Gateway | B | GatewayConfigStore 增量，TLS matcher 全量重建（短路优化） |
| GatewayClass | C | Vec 线性扫描，数量极少可接受 |
| EdgionGatewayConfig | C | 同 GatewayClass |
