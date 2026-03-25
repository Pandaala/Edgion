# 代码质量与性能优化

优先级: P2

---

## ~~1. GRPCRoute match engine 请求时不必要的 String 分配~~ ✅ 已完成

- `parse_grpc_path` 改为返回 `(&str, &str)` 切片引用，零堆分配
- `exact_routes` 从 `HashMap<(String, String), _>` 改为嵌套 `HashMap<String, HashMap<String, _>>`，查找用 `get(service).and_then(|m| m.get(method))`，利用 `String: Borrow<str>` 零分配
- `service_routes` 查找同样零分配
- 请求时匹配路径：从 4 次堆分配降为 0 次

---

## ~~2. TLS proxy 手动双向拷贝~~ ❌ 不改

### 位置

- `src/core/gateway/routes/tls/proxy.rs`（`duplex` 方法）
- `src/core/gateway/routes/tls/passthrough_proxy.rs`（`duplex` 方法）

### Review 结论

两处 `duplex` 实现完全相同（~50 行），但不值得抽取或迁移：

- **抽公共函数收益低**：`TlsProxy` 和 `PassthroughProxy` 是独立 struct，不共享 trait。抽 free function 增加间接层但无实质收益
- **`copy_bidirectional` 不适用**：不支持按方向统计 `bytes_sent`/`bytes_received`，需包装 stream 实现计数，反而更复杂
- **性能无问题**：8KB buffer 是 tokio 默认大小，TLS proxy 瓶颈在握手和网络 IO，不在 buffer 拷贝
- 50 行自包含函数，不复杂，各自跟随 struct 方法更清晰

---

## ~~3. weighted_selector 全零 weight panic~~ ❌ 不需要修

### Review 结论

`WeightedRoundRobin::new` 的 `assert!(total_weight > 0)` 在当前调用链中不会触发：

- 唯一调用方 `BackendSelector::create_state`（`selector.rs:86-103`）在构造前已过滤掉 weight=0 的 item
- 全部 weight=0 时 `valid_items` 为空，返回 `SelectorState::Error(ERR_NO_BACKEND_REFS)`，不会走到 `WeightedRoundRobin::new`
- `select()` 返回 `Err(1)`，调用方处理为错误响应，不会 panic
- `assert!` 作为防御性检查保留即可，没有实际触发路径

---

## ~~4. BackendTLSPolicy target key 精度不足~~ ✅ 已修正注释

### Review 结论

实际风险为零，不需要改 key 格式：

- Gateway API 规范中 `BackendTLSPolicy.targetRef` 只能引用 `group: ""`, `kind: "Service"`，不会出现同 namespace 同 name 不同 kind 的冲突
- 写入（`build_target_key`）和读取（`get_policies_for_target`）都用 `namespace/name`，行为一致
- 调用方 `query_backend_tls_policy_for_service` 本身只查 Service

唯一问题是注释写错了 `namespace/group/kind/name`，已修正为 `namespace/name`
