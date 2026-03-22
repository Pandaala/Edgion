# Circuit Breaker 熔断器 — 第一步：深度研究报告

> 研究日期：2026-03-22
> 研究者：资深工程师（Claude）
> 目标：为 Edgion API Gateway 添加 per-backend 熔断器

---

## 1. 当前状态：现有健康检查和后端状态管理机制

### 1.1 主动健康检查（Active Health Check）

**核心文件：**
- `src/core/gateway/backends/health/check/status_store.rs` — 健康状态存储
- `src/core/gateway/backends/health/check/manager.rs` — 后台探针任务管理
- `src/core/gateway/backends/health/check/probes.rs` — HTTP/TCP 探针实现

**工作机制：**

`HealthCheckManager` 为每个 Service 启动一个后台 Tokio 任务，按固定间隔对所有后端发送 HTTP/TCP 探针。结果写入 `HealthStatusStore`。

`HealthStatusStore` 使用 `service_key -> backend_hash -> BackendHealthState` 的两层结构：
- 内部状态（`consecutive_successes`、`consecutive_failures`）受 `Mutex` 保护
- 对外暴露的不健康后端集合用 `ArcSwap<HashSet<u64>>` 存储，读路径无锁

**关键状态字段（`BackendHealthState`）：**
```
healthy: bool
consecutive_successes: u32
consecutive_failures: u32
last_check_time: Instant
last_transition_time: Option<Instant>
```

**触发逻辑：** 连续失败 >= `unhealthy_threshold` → 标记 unhealthy；连续成功 >= `healthy_threshold` → 恢复 healthy。状态变化时原子更新 `unhealthy_backends` 快照。

**配置来源：** K8s 对象 `annotation: edgion.io/health-check`（YAML 格式），通过 `parse_health_check_annotation()` 解析。

### 1.2 后端运行时状态（Backend Runtime State）

**核心文件：** `src/core/gateway/lb/runtime_state/mod.rs`

`ServiceRuntimeState` 结构：
```rust
pub struct ServiceRuntimeState {
    pub ewma: DashMap<SocketAddr, AtomicU64>,
    pub conn_counts: DashMap<SocketAddr, AtomicUsize>,
    pub backend_states: DashMap<SocketAddr, BackendState>,
}
```

`BackendState` 枚举（`src/core/gateway/lb/leastconn/backend_state.rs`）：
```rust
pub enum BackendState { Active, Draining, Removed }
```

`is_backend_active()` 检查 `BackendState::Active`，目前只用于流量 Drain 场景（优雅下线），与健康检查完全独立。

### 1.3 负载均衡层的健康过滤

**核心文件：** `src/core/gateway/backends/mod.rs`

所有 LB 算法（RoundRobin、ConsistentHash、LeastConn、EWMA）的后端选择都经过同一个 `is_backend_healthy()` 过滤器：

```rust
fn is_backend_healthy(service_key, backend, health_store) -> bool {
    if !health_store.has_service(service_key) { return true; }  // 无健康检查配置则默认健康
    health_store.is_healthy(service_key, backend_hash(backend))
}
```

此函数是熔断器接入的关键 hook 点。

### 1.4 请求失败处理（Pingora Hooks）

**核心文件：**
- `pg_fail_to_connect.rs` — 连接失败 hook
- `pg_error_while_proxy.rs` — 代理中途错误 hook
- `pg_upstream_response_filter.rs` — 上游响应 hook（记录 status_code）

`fail_to_connect` 当前行为：
- `ConnectTimedout` / `TLSHandshakeTimedout` → 设置 status=504
- 其他错误 → status=503
- 若未超过请求超时且 `try_cnt <= max_retries`，设置 `retry=true`

`error_while_proxy` 当前行为：仅决定是否复用连接做 retry，没有失败计数。

`upstream_response_filter` 当前行为：记录 `status_code` 到 ctx，不做失败统计。

**结论：当前没有任何地方对上游失败做累计计数。** 三个 hook 都是"用完即丢"，不与任何持久状态关联。

### 1.5 Webhook Provider 的 Half-Open 实现（参考）

**核心文件：** `src/core/gateway/link_sys/providers/webhook/health.rs`

Webhook 实现了被动健康检查（passive monitoring）+ half-open 探测的完整逻辑：
- `record_passive_result()` — 请求完成后记录成功/失败
- `should_halfopen_probe()` — 指数退避后允许一次探测请求通过
- `healthy` 是 `AtomicBool`，`passive_failures` 是 `AtomicU32`，`backoff_sec` 是 `AtomicU64`

这是 Edgion 内唯一的被动健康检查+熔断器雏形实现，可直接参考迁移到 HTTP backend 侧。

---

## 2. 熔断器设计

### 2.1 状态机

```
         N 次失败 / 在时间窗口内失败率 >= 阈值
Closed ──────────────────────────────────────────────────> Open
  ^                                                          |
  |  M 次连续成功                                           | 冷却时间到期
  |                                                          v
  └────────────────────────────── Half-Open <──────────────
                                   (允许少量探测请求通过)
```

**三态定义：**
- `Closed`：正常转发，统计滑动窗口内的失败情况
- `Open`：熔断中，直接返回 503，不转发任何请求
- `Half-Open`：冷却结束后允许 1 个（或少量）探测请求通过；成功则回 Closed，失败则重置冷却计时器回 Open

### 2.2 失败计数策略

**推荐：滑动时间窗口（Sliding Window）**

理由：
- 简单计数器容易被突发流量触发误熔断（10s 内 5 次失败在低流量场景很严重，但在高流量场景可能是正常波动）
- 滑动窗口基于时间，配置语义更直观（"60s 内失败率 >= 50%"）

**实现方案：固定窗口近似滑动窗口**

为了避免高内存开销，使用两段固定窗口（当前窗口 + 上一窗口的加权平均）来近似滑动窗口，这是业界常见的轻量实现：

```
struct WindowCounter {
    current_requests: AtomicU64,
    current_failures: AtomicU64,
    prev_requests: AtomicU64,
    prev_failures: AtomicU64,
    window_start_secs: AtomicU64,  // epoch seconds
    window_size_secs: u64,
}
```

每次访问时检查是否需要滚动窗口（当前时间超过 window_start + window_size），若超过则将 current 移到 prev，重置 current。

**触发条件（可配置）：**
- 最小请求数：窗口内至少 N 个请求才开始评估（避免低流量下少量失败触发熔断）
- 失败率阈值：窗口内失败率 >= X%
- OR：连续失败次数 >= M（快速触发，适合连接失败场景）

**哪些算作"失败"（可配置）：**
- 连接失败（`fail_to_connect` with non-timeout error）
- 连接超时（`ConnectTimedout`）
- 特定 HTTP 状态码（5xx，可配置具体 code 列表）
- 代理中途错误（`error_while_proxy`）

### 2.3 Half-Open 探测策略

**推荐：基于真实请求的 Half-Open，而非独立主动探针**

理由：
- 与主动健康检查的路径（特定 health check 路径）不同，熔断器关心的是"业务请求是否能成功"
- 避免维护额外的探针线程

实现：Open 状态冷却结束后，设置 `half_open_permitted: AtomicBool = true`。收到请求时，若为 Half-Open 且 `half_open_permitted` 为 true，则 CAS 将其设为 false，允许该请求通过，后续请求继续返回 503，等待本次探测结果。

**与主动健康检查的协作：**
- 两者独立运行，互为补充
- 主动健康检查发现后端不健康 → 直接从 LB 过滤（现有逻辑）
- 熔断器基于实际请求的失败率/次数触发 → 提供更快速的被动保护
- Half-Open 探测成功后可以重置熔断器状态，但主动健康检查的状态独立维护

---

## 3. 与现有代码的集成点

### 3.1 新增：CircuitBreakerStore（类比 HealthStatusStore）

新增 `src/core/gateway/backends/circuit_breaker/store.rs`：

```
CircuitBreakerStore {
    services: DashMap<String, Arc<ServiceCbState>>,  // service_key -> state
}

ServiceCbState {
    backends: DashMap<u64, Arc<BackendCbState>>,  // backend_hash -> state
}

BackendCbState {
    state: AtomicU8,           // 0=Closed, 1=Open, 2=HalfOpen
    window: WindowCounter,
    opened_at_secs: AtomicU64, // 何时进入 Open 状态
    half_open_permitted: AtomicBool,  // Half-Open 探测令牌
}
```

### 3.2 集成点 A：`is_backend_healthy()` 扩展

**文件：** `src/core/gateway/backends/mod.rs`

将 `is_backend_healthy()` 改为同时检查健康检查状态 AND 熔断器状态：

```rust
fn is_backend_healthy(service_key, backend, health_store, cb_store) -> bool {
    // 1. 主动健康检查过滤（现有逻辑）
    if health_store.has_service(service_key) && !health_store.is_healthy(...) {
        return false;
    }
    // 2. 熔断器过滤（新增）
    if cb_store.is_open(service_key, backend_hash(backend)) {
        return false;
    }
    true
}
```

`is_open()` 内部处理 Half-Open 逻辑：若状态为 Open 且冷却时间到期，转换到 Half-Open 并发放探测令牌，此次返回 true（允许请求通过）。

### 3.3 集成点 B：`fail_to_connect` hook 记录失败

**文件：** `src/core/gateway/routes/http/proxy_http/pg_fail_to_connect.rs`

在记录 status 之后，调用熔断器计数：

```rust
// 已有：upstream.status = Some(status_code);
// 新增：
if let Some(backend_addr) = ctx.get_current_backend_addr() {
    if let Some(service_key) = ctx.get_service_key() {
        cb_store.record_failure(&service_key, backend_hash_from_addr(&backend_addr));
    }
}
```

### 3.4 集成点 C：`upstream_response_filter` hook 记录成功/5xx 失败

**文件：** `src/core/gateway/routes/http/proxy_http/pg_upstream_response_filter.rs`

在记录 status_code 之后：

```rust
let status_code = upstream_response.status.as_u16();
// 新增：
if let (Some(service_key), Some(addr)) = (ctx.get_service_key(), ctx.get_current_backend_addr()) {
    let is_failure = cb_config.failure_status_codes.contains(&status_code);
    if is_failure {
        cb_store.record_failure(&service_key, backend_hash_from_addr(&addr));
    } else {
        cb_store.record_success(&service_key, backend_hash_from_addr(&addr));
    }
}
```

### 3.5 集成点 D：`error_while_proxy` 记录失败

**文件：** `src/core/gateway/routes/http/proxy_http/pg_error_while_proxy.rs`

类似 fail_to_connect，记录失败计数。

### 3.6 上下文补充：ctx 需要暴露 backend addr

当前 `EdgionHttpContext` 通过 `upstream_info` 记录了 ip/port（字符串），但没有直接暴露 `SocketAddr` 供熔断器使用。需要在 ctx 中增加当前 backend 的标识（hash 或 SocketAddr）。

可以在 `update_peer_metrics()` 时（`pg_upstream_peer.rs`）将选中的 `SocketAddr` 或其 hash 存入 ctx，供后续 hook 使用。

---

## 4. CRD 设计草案

### 4.1 配置挂载点选择

**推荐方案：HTTPRoute annotation（短期）+ 独立 CRD BackendCircuitBreakerPolicy（长期）**

短期（注解方式，与现有健康检查一致的模式）：
```yaml
metadata:
  annotations:
    edgion.io/circuit-breaker: |
      enabled: true
      windowSizeSeconds: 60
      minRequestCount: 10
      failureRateThreshold: 50      # 百分比，50%
      consecutiveFailureThreshold: 5 # 连续失败次数阈值（快速触发）
      openDurationSeconds: 30        # Open 状态持续时间（冷却时间）
      halfOpenMaxRequests: 1         # Half-Open 阶段允许通过的请求数
      failureStatusCodes: [500, 502, 503, 504]
      countConnectionErrors: true
      countTimeouts: true
```

长期（独立 CRD，参考 Gateway API Policy Attachment 规范）：
```yaml
apiVersion: edgion.io/v1alpha1
kind: BackendCircuitBreakerPolicy
metadata:
  name: my-circuit-breaker
  namespace: default
spec:
  targetRef:
    group: ""
    kind: Service
    name: my-service
  window:
    type: SlidingTime
    sizeSeconds: 60
    minRequests: 10
  triggers:
    failureRatePercent: 50
    consecutiveFailures: 5
  recovery:
    openDurationSeconds: 30
    halfOpenMaxRequests: 1
  failureConditions:
    statusCodes: [500, 502, 503, 504]
    countConnectionErrors: true
    countTimeouts: true
```

### 4.2 配置继承层次

```
GlobalConfig (edgion.yaml)
  → 全局默认（如 circuit_breaker.enabled: false）
  → HTTPRoute annotation（per-route 覆盖）
  → BackendCircuitBreakerPolicy（per-service 覆盖，精度最高）
```

注意：熔断器是 per-backend 粒度的，但配置可以挂在 per-service 或 per-route 层级。

---

## 5. 实现路径（分步骤）

### Step 1：数据结构 + 存储层（无侵入）

新增 `src/core/gateway/backends/circuit_breaker/` 模块：
- `store.rs` — `CircuitBreakerStore`（全局单例）、`ServiceCbState`、`BackendCbState`
- `state.rs` — 状态机枚举 `CbState`、状态转换逻辑
- `window.rs` — 滑动窗口计数器 `WindowCounter`
- `config.rs` — `CircuitBreakerConfig` 结构体（解析来自 annotation 的配置）
- `mod.rs` — 公开 API：`record_failure()`、`record_success()`、`is_open()`、`allow_request()`

### Step 2：配置解析

新增 annotation 解析：`edgion.io/circuit-breaker`，解析逻辑参考 `health/check/annotation.rs`。

在 `http_route_preparse.rs` 中，解析 HTTPRoute 的熔断器 annotation，存入 `ParsedRuleInfo.circuit_breaker_config`。

### Step 3：失败/成功统计 Hook

修改以下文件，在关键路径上调用 `CircuitBreakerStore`：
1. `pg_fail_to_connect.rs` — 连接失败/超时 → `record_failure()`
2. `pg_error_while_proxy.rs` — 代理错误 → `record_failure()`
3. `pg_upstream_response_filter.rs` — 5xx 响应 → `record_failure()`，2xx/3xx → `record_success()`

**前提：** 在 `pg_upstream_peer.rs` 的 `update_peer_metrics()` 中，将选中 backend 的 hash 存入 `EdgionHttpContext.current_backend_hash: Option<u64>`。

### Step 4：LB 过滤集成

修改 `src/core/gateway/backends/mod.rs` 的 `is_backend_healthy()` 函数，加入熔断器状态检查。

修改 `src/core/gateway/lb/selection/mod.rs` 的 `select_by_min_metric()` 函数（EWMA/LeastConn 路径），同样集成熔断器过滤。

### Step 5：Half-Open 逻辑

在 `CircuitBreakerStore.allow_request()` 中实现：
- Open 状态：检查 `opened_at_secs + open_duration_secs <= now`，若到期则 CAS 转换到 Half-Open
- Half-Open 状态：CAS 抢 `half_open_permitted` 令牌，成功则允许，失败则拒绝

`record_success()` 中：若当前为 Half-Open，转换到 Closed，重置计数器
`record_failure()` 中：若当前为 Half-Open，重置 `opened_at_secs`，转换回 Open

### Step 6：Prometheus 指标（见第 6 节）

### Step 7：集成测试

单元测试覆盖：
- 状态机转换（Closed → Open → Half-Open → Closed）
- 滑动窗口计数
- 并发场景（多线程同时 record_failure）
- Half-Open 令牌竞争

---

## 6. 新增 Prometheus 指标

```
# 后端当前熔断器状态（0=Closed, 1=Open, 2=HalfOpen）
edgion_circuit_breaker_state{service, backend} gauge

# 熔断器进入 Open 状态的次数
edgion_circuit_breaker_open_total{service, backend} counter

# 因熔断器拒绝（Open 状态短路）的请求数
edgion_circuit_breaker_rejected_total{service, backend} counter

# 当前滑动窗口内的失败率
edgion_circuit_breaker_failure_rate{service, backend} gauge

# Half-Open 探测结果
edgion_circuit_breaker_halfopen_success_total{service, backend} counter
edgion_circuit_breaker_halfopen_failure_total{service, backend} counter
```

指标更新时机：
- 状态转换时更新 `edgion_circuit_breaker_state` 和计数器
- 每次 `allow_request()` 返回 false（因熔断）时 +1 `rejected_total`
- 定期（或每次 `record_*` 时）更新 `failure_rate` gauge

---

## 7. 下一步：第一个 PR 的 Scope

**PR-1：CircuitBreaker 数据层（纯新增，无侵入）**

目标：建立熔断器状态存储和核心逻辑，不修改任何现有文件。

包含：
1. 新建 `src/core/gateway/backends/circuit_breaker/` 模块（state、window、store、config）
2. 全局单例 `CIRCUIT_BREAKER_STORE`
3. 完整单元测试覆盖状态机和窗口计数
4. `mod.rs` 暴露清晰的公共 API：
   - `record_failure(service_key, backend_hash)`
   - `record_success(service_key, backend_hash)`
   - `is_circuit_open(service_key, backend_hash) -> bool`（含 Half-Open 探测令牌发放）
   - `register_service_config(service_key, CircuitBreakerConfig)`

**不包含：**
- 任何现有文件修改
- Prometheus 指标（留给 PR-2）
- 配置解析集成（留给 PR-3）
- LB 过滤集成（留给 PR-4）

这样 PR-1 是纯净的新增代码，Review 成本低，可以独立验证正确性。

---

## 附录：关键问题解答

**Q: 上游请求失败时，当前在哪里处理？能在哪里 hook 计数？**

A: 失败分三类：(1) `fail_to_connect` — 连接层失败，(2) `error_while_proxy` — 传输层错误，(3) `upstream_response_filter` — 应用层 5xx。三个 hook 都是合适的计数点，但需要在 ctx 中提前存好 backend hash。

**Q: `is_backend_active()` 的状态是如何存储和更新的？可以复用吗？**

A: 存储在 `ServiceRuntimeState.backend_states: DashMap<SocketAddr, BackendState>`，目前只管理 Active/Draining/Removed 三态，用于优雅下线，与健康无关。熔断器应独立存储，不复用这个状态（语义不同）。

**Q: 熔断器需要 per-backend 状态，当前数据结构支持吗？**

A: `HealthStatusStore` 已经是 `service_key -> backend_hash -> state` 的两层结构，熔断器可以照抄这个模式。`backend_hash` 是 `Backend` 的稳定标识（基于 addr 哈希），适合作为 per-backend key。

**Q: Half-Open 探测应该用主动健康检查还是真实请求？**

A: 真实请求。主动健康检查路径（特定 URL）无法准确反映业务请求是否能成功。使用真实请求做 Half-Open 探测，与 Webhook 的被动健康检查实现一致。

**Q: 熔断配置应该放在哪个 CRD？**

A: 短期放 HTTPRoute annotation（`edgion.io/circuit-breaker`），与现有健康检查配置模式一致，实现成本低。长期规划 `BackendCircuitBreakerPolicy` 独立 CRD，参考 Gateway API Policy Attachment 标准。

**Q: Pingora 的 `fail_to_connect`、`error_while_proxying` 能用来做失败统计吗？**

A: 完全可以。Pingora 的 ProxyHttp trait 提供这两个 hook，Edgion 已经实现了对应文件（`pg_fail_to_connect.rs`、`pg_error_while_proxy.rs`），在这两个文件中添加熔断器计数调用即可。注意 `error_while_proxy` 目前有 `// todo need add retry logic?` 的注释，说明这里本来就预留了扩展空间。
