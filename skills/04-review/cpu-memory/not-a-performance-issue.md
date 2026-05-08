# CPU / Memory 常见误判场景

> 来源：2026-03-30 全项目 CPU/Memory 深度 Review（574 个 Rust 源文件），逐一对照源码核实后的结论。

---

## metrics label `.to_string()` 是 API 强制要求

**误判场景**：批评 `record_backend_request()` / `record_request_duration()` 中每次 `.to_string()` 导致不必要的堆分配。

**实际情况**：`metrics` 0.24 的 label 值类型为 `SharedString = Cow<'static, str>`。动态值（gateway name、namespace、route 等均非 `'static`）**必须** 是 `Cow::Owned(String)`，`.to_string()` 是 API 最小满足形式。`Arc<str>` 和 `&str` 方案均因 lifetime 不满足 `'static` 而不可行。metrics 调用的主要开销来自注册表 hash + atomic，而非字符串分配本身。

**判定**：不是性能问题，无需修复。

---

## `get_raw_any` / `lookup_gateway` 不是 JSON 序列化

**误判场景**：认为 `lookup_gateway()`、`lookup_service_cluster_ip()` 调用 `list_json()` 做全量 JSON 序列化后线性扫描。

**实际情况**：
```rust
// route_utils.rs
pub fn lookup_gateway(namespace: &str, name: &str) -> Option<Arc<Gateway>> {
    let processor = PROCESSOR_REGISTRY.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_raw_any(&key)?.downcast::<Gateway>().ok()
}
```
`get_raw_any` 走 `KubeRawStore`（DashMap）或 `FsRawStore`（RwLock<HashMap>），均为 **O(1) 查找**。`downcast` 只做 TypeId 比较，无 JSON 序列化，无线性扫描。

**判定**：代码正确高效，不是性能问题。

---

## HTTP/gRPC route 正则匹配：Controller 已校验 + 热路径无 fallback

**历史误判**：`match_header` / `match_query_param` 中曾有 `compiled_regex` 为 `None` 时调用 `Regex::new()` 的回退分支，被审计反复标记为热路径性能问题。

**当时的辩护**：声称"路由创建时已统一预编译，回退分支只在配置 bug 时走，正常运行不触发"。

**辩护的破绽**：Controller 端 `validate_http_route_if_enabled` / `validate_grpc_route_if_enabled` 当时**只校验 `match_type` 枚举值**，没有真正 `Regex::new()` 试编译。非法正则可以畅通进入数据面，`compile_match_regexes` 用 `Regex::new(...).ok()` 静默吞错，最终每次请求落到 fallback 重复编译且重复失败——既隐藏配置错，又付热路径开销。

**当前修复（2026-04-25）**：
1. Controller 侧 `ref_grant/validator.rs::check_regex_pattern` 在 `validate_*_route_if_enabled` 中对所有 `RegularExpression` 字段（HTTPRoute path/header/query, GRPCRoute method.service/method.method/header）`Regex::new()` 试编译，失败 push 到 errors → 路由 `Accepted=False`。
2. Gateway 侧 `match_unit.rs::match_header` / `match_query_param`（HTTP & gRPC）**删除运行时 fallback**，`compiled_regex == None` 直接返回 `Ok(false)` fail-closed。
3. Gateway 侧 `compile_match_regexes` 仍保留 `Regex::new(...).ok()`（防御 controller bug），失败时 `tracing::error!` 大声告警，不静默。

**审查时认定标准**：
- 看到 `match_header` / `match_query_param` 里没有 `Regex::new` 调用 → 已是修复后状态，不是问题。
- 看到 `compile_match_regexes` 里 `Ok/Err` 双臂带 `tracing::error!` → 这是数据面对 controller bug 的最后防线，不是性能问题。
- 看到 controller `check_regex_pattern` 调用 → 说明非法正则在准入期就被拒，Gateway fallback 路径理论不可达。

**判定**：当前实现里 fallback 已彻底移除；任何"建议给 match_header 加运行时 Regex::new"的 finding 都是反向 regression。

---

## Watcher Vec 懒清理足够

**误判场景**：`watchers: RwLock<Vec<WatchClient<T>>>` 在断开连接后不立即缩容，被标记为内存增长。

**实际情况**：每次注册新 watcher 时执行 `retain(|w| !w.sender.is_closed())` 清理已断开的旧 watcher，新旧 watcher 交替时清理及时。Controller 场景 watcher 数量有限（通常 < 100），即使全部积累也不超过 20KB。在 `notify_watchers` 中增加 retain 会导致每次广播都获取 write lock，反而增加热路径锁竞争。

**判定**：懒清理足够，当前设计合理，不是内存问题。

---

## Webhook 先读后截断有 16KB 硬上限保护

**误判场景**：`read_body_limited` 调用 `resp.bytes().await` 先完整读入内存再截断，被描述为"1GB 全量缓冲"。

**实际情况**：
```rust
// 调用处
let max_bytes = runtime.config.max_response_bytes.min(WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES);
pub const WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES: usize = 16384;  // 16KB 硬上限
```
两层 `.min()` 守卫，最坏情况读入并丢弃的额外内存 = 16KB，而非描述中的 1GB。代码注释已承认此限制并说明"sufficient since max_bytes is capped at 16KB"。

**判定**：影响极小（额外 16KB），不需要修复。

---

## OnceLock 全局单例是刻意设计

**误判场景**：`GLOBAL_CONFIG_CLIENT: OnceLock<Arc<ConfigClient>>` 无法释放，被标记为"热重连无法回收旧 ConfigClient"。

**实际情况**：Gateway 进程生命周期内连接的 Controller 端点是固定的，**热切换 Controller 不是当前功能需求**。`OnceLock` 全局单例是刻意的设计决策。若未来需要热重连，需要系统性地重新设计配置同步架构，而不是简单替换为可变状态（会引入大量并发安全问题）。

**判定**：刻意设计，不是内存问题。

---

## Resource reconcile / 配置同步路径的小对象 clone 不是问题

**误判场景**：在 Controller `resource_processor` / Gateway `conf_handler` 等"配置变更触发"的路径上，看到对单个资源、单条 entry、单个 service 的 `.clone()` / `.to_vec()` / 整 HashMap 拷贝，被标为"P3 不必要的 clone / 应改 Arc<T>"。典型样例：

| 位置 | 模式 | 触发频率 |
|------|------|---------|
| `handlers/secret.rs` 同一 secret 给 init_accumulator + secret_store 各 clone 一次 | Secret 资源 reconcile | 每个 Secret 创建/更新 |
| `lb/lb_policy/policy_store.rs::get(service_key) -> Vec<LbPolicy>` | DashMap entry policies `.to_vec()` | HTTPRoute reconcile |
| `plugins/runtime/plugin_runtime.rs` `add_from_httproute_filters_inner` / `create_request_filter_from_edgion` / `create_upstream_response_filter_from_edgion` 中 `XxxFilter::new(config.clone())` | 入参为 `&[HTTPRouteFilter]` / `&EdgionPlugin`，filter 构造函数按值持有 config | PluginRuntime 构建（reload / 路由变更）|

**实际情况**：这些 clone 全部在 reconcile / config sync 路径，调用频率 = K8s 资源变更频率（人/Operator 操作量级，最高也是 reload 时的 burst），**不是每请求触发**。被 clone 的对象通常是单条资源（KB 量级），整路径耗时由 etcd RTT / gRPC 序列化主导，几次 String/HashMap clone 的 alloc + memcpy 在 totals 里测不出来。

要把它们改 Arc 通常需要：
- 改公开类型签名（`WorkItemResult::obj`、`PolicyStore::get` 返回类型等），cascading 到多处 caller / 测试；
- 或加闭包 API（`with_policies(&self, key, F)`）改变上层用法。

冷路径上没有可观测收益，但带来真实的改动面与回归风险。

特别说明 plugin_runtime 这一行（2026-05-08 新增）：审计常误读为"消除 clone"，实际上：
- 调用链当前形状是 `&[HTTPRouteFilter]` / `&EdgionPlugin` → `match` 出 `&Config` → `XxxFilter::new(config: OwnedConfig)`。从 `&T` 拿 owned `T` **必须** clone。
- 把 clone 移进构造函数（改成 `new(&Config)` 内部 clone）只是位移，alloc 数量不变。
- 真要消除 clone 需要把整个调用链改成按值传递（`Vec<HTTPRouteFilter>` / `Vec<RequestFilterEntry>` / `EdgionPlugin`），级联到 `RouteRuntime` 构建、热更新逻辑、所有 `WrongStage` 分支里仍要借用 plugin 取 name 的写法变化——影响面远超"局部 clone 优化"标签。
- 改 `Arc<Config>` 要动所有 plugin struct/trait，得不偿失。
- 触发频率仅 reload / 路由变更，非请求路径。

**判定**：是否在热路径决定一切——
- ✅ 拒绝："P3 reconcile / config sync 路径上的小对象 clone" 类 finding。
- ✅ 拒绝："plugin_runtime 构建期 config.clone()" 类 finding（API 形状决定，移位不省，全链路按值改动太大）。
- ⚠️ 接受："full_set 整 map 拷贝且每秒同步" 这种放大型场景（已有案例：LinkSys `replace_all` 用 `Arc::clone` 共享给 dispatch task，避免整 HashMap copy；ACME `challenge_store::lookup` 改用 `Bytes` 在 HTTP-01 验证请求每次命中省一次 String alloc + memcpy）。
- 接受口径：finding 必须证明「在请求路径每秒触发」或「单次拷贝 ≥ MB 量级」之一，否则按本条关闭。

---

## OIDC / JWT claims JSON 序列化是 opt-in 功能本身

**误判场景**：批评 `openid_connect/userinfo.rs:60-74` 与 `jwt_auth/verify.rs:172-176` 在每个认证请求上做 `serde_json::to_string(claims)`，建议"延迟序列化"或"文档警告默认开启"。

**实际情况**：

三个相关开关默认全部 `false`：
- `set_userinfo_header` → `openid_connect.rs:340` = `false`
- OIDC `store_claims_in_ctx` → `openid_connect.rs:343` = `false`
- JWT `store_claims_in_ctx` → `jwt_auth.rs:187` = `false`

"用户极易默认开启"不成立。开关一旦打开，序列化即功能本身：

- `X-Userinfo` header 必须在请求转发到上游前写好，下游没有 trigger 点可"惰性化"。
- `set_ctx_var(name, value)` 签名是 `&str`，ctx var 模型存 owned String。要做"读时序列化"得把 ctx var 改成 typed `Value`，影响整个 plugin runtime 的 ctx 接口。
- claims 体积通常 < 1KB，`serde_json::to_string` 微秒级，与请求路径上的网络/路由匹配开销不在一个量级。

唯一可见的微优化：`set_userinfo_header && userinfo_json.is_none() && store_claims_in_ctx` 三者同时成立时 `userinfo.rs:60` 与 `71` 会重复序列化一次——可复用同一 String。但触发条件窄、省一次微秒级 alloc，改动收益不抵 review/回归成本。

**判定**：opt-in 功能的固有成本，不是性能问题。任何"OIDC/JWT 热路径 claims JSON 序列化"类 finding 按本条关闭，除非证明默认值已改为 `true` 或单次 claims ≥ 数十 KB。

---

## 离线 Controller 数据保留是业务需求

**误判场景**：`ControllerRegistry` / `ResourceAggregator` 中离线 Controller 的条目不被定期清理，被标记为"内存泄漏"。

**实际情况**：离线 Controller 的注册信息和资源快照是**需要持久保留的业务数据**——Admin API 需要展示离线状态，运维人员需要查看离线前的资源快照。清理走显式运维路径：`DELETE /api/v1/center/admin/controllers/{id}` 级联删除四个内存结构 + DB 行（见 `skills/09-misc/center-db.md` 的 Admin API 表）。活资源（`stream_tx` / forwarding task / tonic HTTP/2 stream）则在 `mark_offline` 时立即释放，不等 DELETE。

> 历史注意：早期有 `evict_stale(hours)` 方法 + `offline_evict_hours` 配置，但从未被 scheduler 调用，是死代码。已在 2026-04 的 fix #19 中移除；当前语义即"离线数据无 TTL，仅通过 Admin DELETE 清理，但活资源离线瞬间就释放"。

---

## `extract_status_json` 双次序列化是冷路径 + API 形状决定

**误判场景**：`processor.rs:process_resource_inner` 在 `update_status()` 前后各调用一次 `extract_status_json(&parsed_obj)`，每次内部 `serde_json::to_value(obj)` 完整序列化整个对象再 `.get("status")`，被标记为"重复序列化、HTTPRoute 多 rules 时 CPU 显著上升"。

**实际情况**：

- **冷路径**：`process_resource_inner` 只在 K8s 资源 watch event 触发，不是请求路径。watch 事件频率远低于请求，且本来就异步排队执行。
- **第二次序列化绕不开**：两次调用之间 `self.handler.update_status(&mut parsed_obj, ...)` 会修改 status，没法复用第一次的 `Value`。报告里的"方案 A：序列化一次复用 Value"在语义上不成立。
- **真正可省的是"序列化整个对象"→"只序列化 status 子树"**，但 `extract_status_json<T: Serialize>` 是泛型函数，只要求 `Serialize`，无法直接访问 `.status` 字段。要优化必须给 K 资源 trait 加 `status_value() -> Option<Value>` 方法（影响 trait 定义 + 所有 impl）；或为每种 status 实现 `PartialEq` 并重写 `status_semantically_changed`（当前依赖 JSON 比较以忽略 `lastTransitionTime` 等非语义字段，重写成 struct 级比较改动面更大）。
- 比较函数 `status_diff::status_semantically_changed(kind, old, new)` 现签名收 JSON 字符串，干掉序列化要级联改它的语义比较逻辑。

**判定**：冷路径 + API 形状决定，改动面（trait 扩展 / 比较语义重写）远超收益。任何"`extract_status_json` 重复序列化"类 finding 按本条关闭，除非实测 profile 显示 controller CPU 时间显著占比落在此函数。

**判定**：非内存泄漏，是业务语义保留；活资源已即时释放，保留的只是静态注册快照。

---

## 配置 struct 字段排序导致的 padding 浪费

**误判场景**：审计标记 `OpenidConnectConfig`（`src/types/resources/edgion_plugins/plugin_configs/openid_connect.rs:78-227`）等带 `JsonSchema` 的配置 struct，bool 与 String/u64/Option 交错排列，每实例浪费 30-50 B 对齐 padding，建议按字段大小重排以紧密打包。

**实际情况**：

1. **量级可忽略**：插件 config 按路由级实例化，几百个实例 × 30-50 B = 十几 KB 总浪费，profile 测不出。
2. **非热路径**：config 在 reconcile 阶段加载一次，请求路径只读少量字段，cache line 利用率与 padding 无关。
3. **字段顺序是用户可见 schema 的一部分**：这类 struct 通常带 `#[derive(JsonSchema)]` + `serde(rename_all = "camelCase")`，字段顺序直接映射到生成的 CRD / JSON Schema 文档。按对齐重排会让外部 schema 出现「bool 先 / String 后」的反直觉顺序，新增字段时还要在「padding 最少」vs「逻辑分组对」之间二选一。
4. **可读性 >> padding**：当前 struct 按 Required / Auth behavior / OIDC params / Token verification / Header forwarding / Network / Runtime / Security extras 分段，维护者一眼能定位字段类别。打散分组换十几 KB 不可观测的内存收益完全不划算。

**判定**：不是内存问题。任何「配置 struct 字段排序优化 padding / 按大小重排」类 finding 按本条关闭，除非（a）该 struct 在请求路径每秒分配/拷贝 ≥ 千次，或（b）单实例 padding ≥ KB 量级。

---

## reconcile / 全量同步路径上的 `HashMap::new()` 默认容量

**误判场景**：审计标记 Center `metadata_store::full_set`（`src/core/center/metadata_store/mod.rs:182-184` 的 `new_cluster` / `new_service` / `new_gir`）以及其他 `full_set` / reconcile 路径上的 `HashMap::new()` 没有用 `with_capacity(data.len())`，建议预分配以减少 rehash。

**实际情况**：

1. **冷路径**：所有 `full_set` 实现位于 Controller reload / Gateway 配置同步 / Center fed_sync 全量推送路径，触发频率 = 事件级（Controller 重连、reload、全量重推），**不是请求级**。
2. **规模极小**：典型场景下 `data.len()` 是单 Controller 的 PluginMetaData / 单类资源数量，通常几十条（见已删除条目 H6 的复核结论）。HashMap 从 0 长到 64 经历 ~3-4 次 rehash，总成本 O(64)，几微秒级。
3. **改动面与收益不匹配**：项目里 `full_set` 实现 20+ 处，其他 reconcile / 配置构建路径上的 `HashMap::new()` 数量更多。批量改 `with_capacity(data.len())` 涉及大量签名/调用点改动，profile 上完全测不出收益——属于已判定为"不是问题"的「reconcile / config sync 路径上的小对象操作」同一类（见上一条 "Resource reconcile / 配置同步路径的小对象 clone 不是问题"）。

**判定**：不是性能问题。任何"reconcile / full_set / 配置同步路径上 `HashMap::new()` 应改 `with_capacity`"类 finding 按本条关闭，除非证明（a）该 HashMap 在请求路径每秒分配 ≥ 千次，或（b）单次 `data.len()` ≥ 万级且 rehash 成本可观测。

---

## Access log JSON 序列化每请求新分配 String 不是问题

**误判场景**：`src/core/gateway/observe/access_log/entry.rs:103-110` `AccessLogEntry::to_json()` 在请求任务上下文执行 `serde_json::to_string(self)`，每请求新分配一个 owned `String`，被标为"热路径每请求 alloc，应做 buffer pool / 推迟到消费线程 / 改 `Bytes`/`Arc<str>`"。

**实际情况**：

1. **架构是 channel-based async logger**：调用链 `routes/http/proxy_http/pg_logging.rs:42-63` → `AccessLogger::send(String)` → `LocalFileWriter` 内部 `SyncSender<String>::try_send`（`link_sys/providers/local_file/data_sender_impl.rs:30,184-187`，满则丢），后台 `std::thread::spawn` 消费写盘。**channel 边界形态就是 owned `String`**，请求任务上必然有一次 owned 化。

2. **序列化无法挪到消费线程**：`AccessLogEntry<'a>` 通篇借用请求上下文（`request_info`、`backend_context`、`stage_logs`、`ctx_map` 等，见 entry.rs:26-62）。要把 JSON 化挪到消费线程等同于先把这些字段全部 clone 过去——alloc 反而更多，生命周期还复杂化。

3. **单次成本可忽略**：单条 entry 1-5 KB，jemalloc size-class 命中 + 单次 String 分配，绝对成本 < 微秒级。同一请求路径上 serde 遍历 + plugin/route 执行 + upstream I/O 都在 ms 级，profile 测不出此处 alloc。

4. **"省 alloc"方案都不真省**：
   - thread-local `Vec<u8>` + `serde_json::to_writer`：仍要 `String::from(buf)` 塞进 channel，alloc 数不变。
   - `Bytes` / `Arc<str>`：`DataSender<String>` trait + `LocalFileWriter::SyncSender<String>` + 所有 logger（access / mirror / debug / tcp / udp）全得改，alloc 没省。
   - 推迟到消费线程：必须 clone 全部借用字段，得不偿失。

5. **真正瓶颈在下游**：channel 满即丢，磁盘 I/O 才是天花板。`to_json()` 减半不会改变吞吐。

**判定**：不是性能问题。和"OIDC/JWT claims JSON 序列化是 opt-in 功能本身"同型——access log 一旦开启，**JSON 序列化就是功能契约**，channel 边界 owned String 是 API 形状决定。任何"access log `to_json()` 应做 buffer pool / 改 `Bytes` / 推迟到消费线程"类 finding 按本条关闭，除非证明（a）单条 entry ≥ 数十 KB 且高 QPS，或（b）profile 实测 `to_json` alloc 在请求路径有显著占比。

---

## CenterDb 单 `Mutex<Connection>` 不是写入阻塞问题

**误判场景**：`src/core/center/db/mod.rs:28-30` 的 `CenterDb { conn: Arc<Mutex<rusqlite::Connection>> }` 被标记为"全 Center 共享单连接，admin 列表查询与注册写入互阻塞"，建议引入 r2d2 连接池或写连接 + 多读连接 + WAL。

**实际情况**：

1. **触发频次是事件级/运维级，不是请求级**：
   - `upsert_controller` → Controller 注册/重连（每个 Controller 启动一次或网络波动时偶发）
   - `mark_offline` → 心跳超时 / 断流（Controller 维度稀疏事件）
   - `delete_controller` / `list_controllers` → Admin API（运维手动）
   
   数据面请求路径完全不碰这把锁。1000 个 Controller 同时重连这种极端场景下，单次 upsert ≈ 几十微秒，全部串行 ≪ 1s。

2. **SQLite 本身就是单写**：默认 rollback journal 模式下，多连接写依然要串行竞争文件锁。换 r2d2 池只是把等待从 Mutex 挪到 SQLite 内部，写侧零收益。WAL 模式能让读不阻塞写，但本场景读频次（admin LIST）也极低，收益不可观测。

3. **写路径已 spawn_blocking 隔离**：`fed_sync/server/mod.rs:171, 307` 与 `api/mod.rs:248, 295` 全部在 `tokio::task::spawn_blocking` 里跑，Tokio runtime 线程不会被 Mutex 阻塞；blocking 池默认 512 线程对 sub-ms 级操作完全够用。

4. **代码自身已定位为 best-effort**：`server/mod.rs:161-164` 注释明确"Persist registration to SQLite (best-effort, isolated from the hot path). Any failure here is logged and swallowed — we refuse to block fed-sync registration on DB availability"——DB 写从来不在关键路径上，失败都吞掉。

5. **数据规模有限**：表只有 `controllers` 一张，行数 = Controller 数（通常 < 1000），所有 query 都是按 PK 单行 upsert/delete 或全表 select。

6. **改动面不抵收益**：r2d2 + WAL 需要新依赖、checkpoint 协调、测试重写，profile 上根本看不到这把锁。

**判定**：不是性能问题。任何"CenterDb 单 Mutex 阻塞 / 应改连接池或 WAL"类 finding 按本条关闭，除非证明（a）DB 操作进入了请求路径，或（b）profile 实测此锁有显著等待时间。

---

## Admin API `controller_summaries()` 多次调用 / 部分字段丢弃不是问题

**误判场景**：审计标记 `ResourceAggregator::controller_summaries()`（`src/core/center/aggregator/mod.rs:123-135`）在多个 Admin API handler 中被调用，且部分调用点只取一两个字段就丢弃其余字段，被判为"重复 clone 全字段、CPU 浪费"。典型样例：

| 调用点 | 实际取用字段 | "丢弃"字段 |
|-------|------------|-----------|
| `api/mod.rs:178` `list_controllers` | 全部 5 字段 | 无 |
| `api/mod.rs:183` `list_clusters` | 仅 `.cluster` | controller_id / env / tag / online |
| `global_connection_ip_restriction_handlers.rs:53,161` `online_controllers` | filter `.online` + 取 `.controller_id` | cluster / env / tag |
| `region_route_handlers.rs:150,250,312` | 按 controller_id / online 部分取 | 同上 |

**实际情况**：

1. **全部是 Admin API，非请求路径**：调用源是运维 dashboard / CLI（list controllers / clusters / region routes / global IP restrictions），调用频次 = 人/Operator 操作量级，不是数据面请求级。
2. **规模有限**：单 Center 下 controller 数量典型几十到几百（远小于千级）。`ControllerSummary` 仅含 2 个短 String + 2 个常空或极短 `Vec<String>` + 1 个 bool，单次 `collect` 顶多几 KB。
3. **多次调用不互阻塞**：`controller_summaries` 内部是 `RwLock::read()`，read 锁非独占，多 handler 并发调用无锁竞争。
4. **要"省"得改 API 形状**：要消除全字段 clone，得在 `ResourceAggregator` 上加 `cluster_names()` / `online_controller_ids()` 等专用方法（API 表面膨胀 + 多份重复实现），或改成 `with_summaries(F)` 回调（call site 全改 + 读锁持有时间被回调控制反而增加并发风险）。profile 上完全测不出收益。

**判定**：不是性能问题。任何"`controller_summaries` 多次调用 / 部分字段丢弃应改专用方法或回调"类 finding 按本条关闭，除非证明（a）该方法被改造为请求路径每秒触发，或（b）controller 数量已 ≥ 万级。

---

## `CenterWatchCacheRegistry::get_or_create` 慢路径 `to_string()` 不是问题

**误判场景**：`src/core/center/watch_cache/registry.rs:38-39` 在写锁分支里 `controller_id.to_string()` 出现两次（一次作 HashMap key，一次传给 `CenterWatchCache::new` 持有），被标为"反复 stringify，应改 `Arc<str>` / 字符串 intern"。

**实际情况**：

1. **慢路径，每个 controller 一生一次**：`get_or_create` 的 fast path 是 `caches.read()` + `Arc::clone`，完全不碰 `to_string()`。两次 `to_string()` 只在写锁分支首次插入时执行，对应 controller 注册或断线重连——典型集群是几个到几十个 controller 量级，整体只发生几十次。
2. **不在请求路径**：调用源是 `fed_sync::server` 收到 `RegisterRequest` 后调一次，不是 per-event / per-request。
3. **绝对成本可忽略**：每次 2 次小 String 分配（controller_id 通常 几十字节），远低于单次 gRPC 注册请求的 protobuf 反序列化与 SQLite upsert 开销。
4. **`Arc<str>` 救不了多少**：最多省掉 1 次分配（HashMap key 仍要独立持有 owned key 类型，除非把 `HashMap<String, _>` 一并改成 `HashMap<Arc<str>, _>`，那是更大的级联改动）。intern 池更不适用——这里不是"同一字符串 stringify 上千次/秒"的场景。

**判定**：不是性能问题。任何"`get_or_create` / registry 注册路径上 controller_id `to_string()` 应改 `Arc<str>` / intern"类 finding 按本条关闭，除非证明该路径已被改造为 per-request 触发或单次注册 ≥ 数千 controller。

---

## Route handlers `record_cross_ns_refs` 循环 `add_ref` 不是锁热点

**误判场景**：审计标记 `record_cross_ns_refs`（`resource_processor/handlers/{http,udp,grpc,tcp,tls}_route.rs:67-90` 同模式）对每个 cross-namespace `backend_ref` 调用 `manager.add_ref(...)`，每次拿一次写锁，建议引入 `batch_update` 一次锁内批量插入。

**实际情况**：

1. **冷路径——K8s 资源 watch event 触发**：调用源是 Route 资源 reconcile，事件级（operator 操作量级），不是请求级。同函数前一行 `manager.clear_resource_refs(...)` 本身就是一次写锁。
2. **锁是 `parking_lot::RwLock<RefMaps<V>>`**（`ref_manager.rs:170`），uncontended write 约 25 ns；单条路由典型 backend_refs 1-10 个，pathological 100 个 → 总锁开销 ~25 ns–2.5 µs。整路径耗时由 etcd RTT + 资源解析 + status update 主导，几次锁获取在 totals 里测不出来。
3. **改动面 vs 收益**：要"省"得在 `BidirectionalRefManager` 加 `add_refs_batched` / `batch_update` 方法，或暴露 inner 让 caller 自管锁——破坏封装、API 表面增大、5 个 handler（http/udp/grpc/tcp/tls）全改。冷路径上零可观测收益。
4. 与已有"Resource reconcile / 配置同步路径的小对象 clone 不是问题"同类——reconcile 事件级 + 规模小 + 改动面级联。

**判定**：不是性能问题。任何"`record_cross_ns_refs` / 类似 reconcile handler 中循环单次锁应改 batch / 单次锁内完成"类 finding 按本条关闭，除非证明（a）该路径被改造为请求级触发，或（b）单条资源 backend_refs ≥ 千级且 profile 实测锁等待显著。

---

## `BidirectionalRefManager` 无 GC/过期机制不是内存泄漏

**误判场景**：审计标记 `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_manager.rs` 的 `BidirectionalRefManager`（forward `source_key → Set<V>` + reverse `value_key → Set<source_key>`）没有 `gc` / `cleanup` / `purge` / `expire` 方法，"删除事件丢失或处理失败时引用会永久残留"，建议加 `gc(existing_values)` 周期任务定期校验。典型样例：`SecretRefManager`、`ServiceRefManager`、`CrossNamespaceRefManager`、`BackendTLSPolicyTargetManager`、`stream_plugin_ref_mgr`。

**实际情况**：

1. **它不是缓存，是对当前 K8s 资源图的索引**：条目生命周期完全跟随 K8s 资源，没有独立 TTL/过期语义。把它当通用 cache 套用"必须有 GC"是范畴错误。

2. **每个 handler 走 clear-then-readd 模式**：所有 `process_*` 路径都是先 `manager.clear_resource_refs(&resource_ref)` 再循环 `add_ref(...)`。覆盖文件：`handlers/{http,grpc,tls,tcp,udp}_route.rs:74` + `handlers/{http,grpc,tls,tcp,udp}_route.rs:~210/229`（cross_ns）、`edgion_tls.rs:111/234`、`edgion_acme.rs:116/277`、`edgion_plugins.rs:82/135`、`backend_tls_policy.rs:130/178/300/306`、`gateway.rs:236/239/262/498/499/507`、`link_sys.rs:126/166`、`route_utils.rs:423/450`。即便中间漏报某个事件，下一次该资源的 reconcile 会自然把陈旧引用清掉。

3. **每个 handler 的 delete 路径也调用 `clear_resource_refs`**：与 process 路径成对（同上文件第二个调用点）。

4. **Watch 漏事件有兜底**：Kubernetes informer 自带周期 resync；watch 重连会以 DELETED 重放丢失的删除事件，handler 再次走 clear。

5. **Leader 切换 / Controller 重启全量重置**：`runtime.rs:123 reset()` 显式 `service_ref_mgr.clear()` + `cross_ns_ref_mgr.clear()` + `processor_registry.clear_processors()`，随后 init full-sync 从 K8s 重建索引。任何长期累积的"残留"在重启后归零。

6. **内存有界**：总引用数 = Σ(每个 K8s 资源持有的引用数)，被资源图大小直接限制。`Set<V>` 自带去重（`add_ref` 幂等），`remove_ref` / `clear_value_refs` 在 set 空后还会 `HashMap::remove` 收回 bucket（`ref_manager.rs:216-218,222-224,268-271`）——没有无界增长路径。

7. **建议的 `gc(existing_values)` 周期任务有副作用**：reconcile 是 clear-then-readd 两阶段，GC 与 reconcile 并发时可能在 clear 之后、add 之前的窗口里把"现存但暂时不在索引里"的 value 删掉，反而引入 bug。即便加同步原语，也是为不存在的问题付锁开销与 reconcile 协调复杂度。

**判定**：不是内存问题。任何"`BidirectionalRefManager` / `SecretRefManager` / `ServiceRefManager` / `CrossNamespaceRefManager` 应加 GC / 过期 / 周期校验"类 finding 按本条关闭，除非证明（a）handler 漏掉 delete 路径的 `clear_resource_refs` 调用（属个案 bug，修该处而非加 GC），或（b）实测在长时间稳态下 `stats().total_references` 持续单调增长且与 K8s 实际资源数明显脱节。

---

## Workqueue 延迟堆无显式大小上限不是问题

**误判场景**：`src/core/controller/conf_mgr/sync_runtime/workqueue.rs:330-384` 的 `spawn_delay_loop` 中 `BinaryHeap<DelayedItem>` 没有显式 capacity / 大小检查，被标记为"无界增长可被滥用 / 内存放大风险"，建议加 `max_heap_size` 拒绝或淘汰策略。

**实际情况**：

1. **入堆前已按 key 去重**：`enqueue_after`（workqueue.rs:531-539）首先 `self.scheduled.insert(key)`，已在 `scheduled` `DashSet` 中则 `return false`，根本不发到 `delay_tx`。同一 key 在堆里最多一个条目，直到 promote 时（workqueue.rs:357 `scheduled.remove`）才允许再次调度。

2. **每个 Workqueue 只服务一个资源类型**（`workqueue.rs:250` "Each resource type should have its own workqueue instance"），堆大小天然受限：
   ```
   |heap| ≤ |该 Kind 下已调度但未到期的 unique key 数| ≤ 集群中该 Kind 的对象总数
   ```
   这个上界由 K8s apiserver/etcd 自身限制，通常几千量级。

3. **delay_tx 通道本身有界**：`mpsc::channel(config.capacity)`（默认 5000，workqueue.rs:191、295）—— 即使堆理论可装更多，sender 端 `delay_tx.send(...).await` 会先阻塞反压。

4. **已有压力告警**：workqueue.rs:541-551 在 `pending + scheduled ≥ 80% capacity` 时打 warn，运维侧可观察。

5. **触发链有循环/深度上限**：`max_trigger_cycles=5`、`max_trigger_depth=20`（workqueue.rs:193-194）阻断 cascade 失控放大；`promote_delayed_item` 触达上限时直接丢弃。

6. **唯一会让堆异常增长的场景是某资源类型在集群里真有百万对象**——但那时 informer/cache 早就先爆了，给堆单独加 cap 解决不了根因。

**判定**：不是内存/性能问题。任何"workqueue delay heap 应加 max_size / 容量检查 / LRU 淘汰"类 finding 按本条关闭，除非证明（a）`scheduled` DashSet 的 dedup 失效（属个案 bug，修 dedup 而非给堆加 cap），或（b）profile 实测稳态下堆 size 与该 Kind 资源对象数明显脱节。

---

## conf_sync watcher 三层 mpsc(100) 串联不是积压问题

**误判场景**：`conf_sync/cache_server/cache.rs:221,299` 与 `conf_sync/conf_server/grpc_server.rs:305` 三处 `mpsc::channel(100)`，被算作"per watcher × 3 channel × 100 capacity × N event = 4w+ event 积压"，建议下调容量或改无锁队列以缓解内存。

**实际情况**：

1. **channel item 是 batched `WatchResponse<T>` 不是单个 event**：`cache.rs:140` watcher task 通过 `EventStore::get_events_from_sync_version`（`store.rs:187`）一次性打包从 `from_version` 起所有未发送事件作为一个 `WatchResponse` 提交。channel 容量 100 = 100 个 *batch*，乘法到 event 数级别本身就是错位单位。

2. **三层 mpsc 全 `.await` 提交，backpressure 全程有效**：
   - typed: `cache.rs:140` `sender.send(response).await`
   - simple: `cache.rs:321` `simple_tx.send(simple_response).await`
   - grpc 出站: `grpc_server.rs:331` `tx.send(Ok(response)).await`
   
   下游堵塞 → 上游阻塞 → watcher task 不再从 EventStore 拉新事件。没有 `try_send`、没有无界 buffer、没有事件丢失旁路。

3. **EventStore 才是 source-of-truth 且有界**：`store.rs:153` 是 circular queue（默认 capacity 200/kind，`mut_update` 满了直接覆盖最旧 slot 并推进 `expire_sync_version`）。watcher 慢消费时事件不会在 channel 多份复制堆积——producer 卡在 `send().await` 时新事件覆盖循环队列，下次该 watcher 唤醒发现 `from_version < expire_sync_version`，触发 `WATCH_ERR_TOO_OLD_VERSION`（`store.rs:194`）让 client 走 relist 重对齐。这是设计好的 fail-safe，不是内存泄漏。

4. **稳态 channel 接近空**：`cache.rs:169` `notify.notified()` driven，watcher 被唤醒一次拉一批回到等待。只有当某个 gRPC client 对端真的网络卡死或慢消费时，对应 watcher 链路的三层 channel 才会涨；这本身是网络故障的症状，不是 channel 容量决定的内存问题。

5. **三层解耦各有独立动机，不是无脑层叠**：
   - typed → simple：把 JSON 序列化从 store read lock 下挪出去（避免长持读锁）
   - simple → grpc：注入 server reload 检测（`grpc_server.rs:339` `check_interval` 周期检查 server_id 变化并主动通知 client relist）
   
   合并任何两层都会引入新约束（在锁内序列化 / reload 通知混入 watcher 主循环）。容量 100 在每层都是经验值的"留点缓冲吸收瞬时抖动"，不是被滥用的无脑大值。

6. **worst-case 边界可控**：per-watcher 三层全满 = 3 × 100 batch × ≤capacity event/batch × per-event 几 KB ≈ 数十 MB，且需要该 watcher 下游连接彻底卡死。N 个 watcher 同时全卡是网络/集群级故障场景，不是 channel 容量的设计缺陷；真发生时也会被 `WATCH_ERR_TOO_OLD_VERSION` + relist 自我修复。

**判定**：不是内存问题。任何"conf_sync watcher channel(100) 串联会积压 N 万 event / 应下调容量 / 应改无锁结构"类 finding 按本条关闭，除非证明（a）有 `try_send` 或无界 buffer 旁路绕过 backpressure，或（b）EventStore 循环队列的 `WATCH_ERR_TOO_OLD_VERSION` relist 路径失效，或（c）profile 实测在正常网络下 channel 长期接近 100 满。
