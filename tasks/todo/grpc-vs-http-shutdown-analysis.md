# gRPC 关闭 vs HTTP 关闭 — 深度分析与问题清单

> 对 Edgion 三大组件（Controller、Center、Gateway）的优雅关闭流程进行全面审查，
> 重点对比 gRPC (tonic) 与 HTTP (axum) 的关闭行为差异，并识别潜在问题。
> Date: 2026-03-30

---

## 一、架构总览：三个组件的服务拓扑

| 组件 | gRPC 服务 | HTTP 服务 | 信号处理 |
|------|-----------|-----------|----------|
| **edgion-controller** | ConfigSync (tonic, `serve_with_shutdown`) | Admin API (axum, `with_graceful_shutdown`) | `ShutdownHandle` 统一协调 (SIGINT + SIGTERM) |
| **edgion-center** | FederationSync (tonic, 裸 `serve`) | Admin API (axum, 裸 `serve`) | 仅 `tokio::signal::ctrl_c()` |
| **edgion-gateway** | 无 | Admin API (axum, 裸 `serve`) + Metrics (axum, 裸 `serve`) | Pingora 内置信号处理 |

---

## 二、gRPC 关闭 vs HTTP 关闭：协议层差异

### 2.1 tonic (gRPC) 的 `serve_with_shutdown`

底层机制：tonic 基于 hyper，当 shutdown signal future 完成时：

1. **停止 accept 新连接**：TCP listener 关闭
2. **HTTP/2 GOAWAY**：向所有已连接客户端发送 GOAWAY 帧，通知不再接受新的 stream
3. **等待 in-flight RPC 完成**：已建立的 stream 继续执行直到完成
4. **无内置超时**：如果某个 streaming RPC（如 `watch`、`watch_server_meta`）一直不结束，tonic 会无限等待

关键特性：
- gRPC 使用 HTTP/2 多路复用，单个 TCP 连接上可能有多个并发 RPC
- GOAWAY 帧让客户端知道需要重新连接到其他实例
- **streaming RPC 是最大隐患**——长连接的 server-streaming 不会因为 GOAWAY 自动结束

### 2.2 axum (HTTP) 的 `with_graceful_shutdown`

底层机制：axum 同样基于 hyper，当 shutdown signal future 完成时：

1. **停止 accept 新连接**：TCP listener 关闭
2. **HTTP/1.1**：等待当前请求完成后关闭连接（不处理 keep-alive 上的下一个请求）
3. **HTTP/2**：发送 GOAWAY，等待 in-flight 请求完成
4. **无内置超时**：与 tonic 相同

关键特性：
- HTTP 请求通常是短暂的 request-response 模式，关闭速度通常远快于 gRPC streaming
- Admin API 全部是 REST 请求，延迟极低，关闭几乎瞬时

### 2.3 核心差异总结

| 维度 | gRPC (tonic) | HTTP (axum) |
|------|-------------|-------------|
| 协议 | HTTP/2，多路复用 | HTTP/1.1 为主 |
| 关闭信号 | GOAWAY 帧 | 停止 keep-alive |
| Streaming 支持 | 有 server-streaming RPC | 无长连接流（SSE 除外） |
| 关闭耗时 | 取决于最慢的 streaming RPC | 通常 < 1s |
| 超时机制 | 无内置超时 | 无内置超时 |
| 客户端感知 | GOAWAY 通知重连 | Connection: close 或断连 |

---

## 三、各组件详细流程分析

### 3.1 edgion-controller（设计最完善）

#### 信号处理链

```
OS Signal (SIGINT/SIGTERM)
  → ShutdownHandle::wait_for_signals()  [spawned background task]
    → self.shutdown()  [watch::channel send true]
      → ShutdownSignal::wait() resolves
        ├── grpc_shutdown future completes
        │     → tonic serve_with_shutdown 开始关闭
        ├── admin_shutdown future completes
        │     → axum with_graceful_shutdown 开始关闭
        └── conf_mgr_shutdown
              → ConfCenter / K8s controllers 停止
```

#### 关键代码路径

**入口**：`src/core/controller/cli/mod.rs`

```
ShutdownHandle::new()
  ├── spawn(wait_for_signals)        ← 监听 SIGINT + SIGTERM
  ├── spawn(conf_mgr.start_with_shutdown)  ← 共用同一 handle
  ├── spawn(fed_client::run)         ← ⚠️ 使用独立 watch::channel
  └── start_services()
       ├── grpc_shutdown = handle.signal().wait()
       ├── admin_shutdown = handle.signal().wait()
       └── tokio::join!(grpc, admin)  ← 并行关闭，等待两者都完成
```

#### 评价

优点：
- gRPC 和 HTTP 共享同一个 `ShutdownHandle`，信号来源统一
- `tokio::join!` 确保两个服务都完成后才退出
- `ConfMgr` 也接入同一 `ShutdownHandle`，K8s controller 能感知关闭

### 3.2 edgion-center（问题最多）

#### 信号处理链

```
Ctrl-C (仅 SIGINT)
  → tokio::select! 捕获
    → shutdown_tx.send(true)  ← 仅通知 scheduler
    → 函数返回 Ok(())
      → spawned gRPC/HTTP tasks 被 tokio 运行时 drop ← 非优雅！
```

#### 关键代码路径

**入口**：`src/core/center/cli/mod.rs`

```
let grpc_handle = tokio::spawn(
    tonic::Server::builder()
        .serve(grpc_addr)       ← 裸 serve，无 graceful shutdown
);

let http_handle = tokio::spawn(
    axum::serve(listener, router)  ← 裸 serve，无 graceful shutdown
);

tokio::select! {
    _ = ctrl_c() => { shutdown_tx.send(true); }  ← 仅通知 scheduler
    r = grpc_handle => { ... }
    r = http_handle => { ... }
}
// 函数返回后，未完成的 spawned tasks 被 abort
```

### 3.3 edgion-gateway（依赖 Pingora 内置机制）

#### 信号处理链

```
OS Signal (SIGTERM/SIGINT)
  → Pingora 内置信号处理
    → grace_period_seconds (默认 30s)
      → is_process_shutting_down() = true
        ├── pg_early_request_filter: 禁用 keepalive
        └── pg_response_filter: 添加 Connection: close
      → graceful_shutdown_timeout_seconds (默认 10s)
        → 强制终止
  → run_server() 返回
    → main 线程结束
      → 后台 tokio runtime 线程被 OS 终止 ← 非优雅！
```

#### 关键代码路径

```
// 这些服务在 tokio::spawn 中启动，无 graceful shutdown
tokio::spawn(api::serve(config_client, 5900));      ← Admin API
tokio::spawn(metrics::serve(&metrics_cfg));          ← Metrics API

// Tokio runtime 移到后台线程
std::thread::spawn(|| runtime.block_on(pending()));

// Pingora 阻塞主线程
run_server(pingora_server);  // server.run_forever()
// 当 Pingora 关闭后，main 返回，进程退出
// 后台线程和 spawned tasks 未被通知
```

---

## 四、发现的问题

### 🔴 P0 — 严重问题

#### 问题 1：edgion-center gRPC/HTTP 无优雅关闭

**位置**：`src/core/center/cli/mod.rs:76-87`

**现象**：
- gRPC 使用 `tonic::Server::serve()` 而非 `serve_with_shutdown()`
- HTTP 使用 `axum::serve()` 而非 `with_graceful_shutdown()`
- 收到 Ctrl-C 后，`tokio::select!` 选中 ctrl_c 分支，函数返回
- spawned 的 gRPC/HTTP tasks 被 tokio 运行时 drop，连接被强制断开

**影响**：
- Federation gRPC stream 被强制中断，控制器客户端无法区分是网络故障还是正常关闭
- HTTP 正在处理的请求可能返回不完整的响应
- 数据不一致风险：如果 command 正在执行过程中被中断

**建议修复**：
1. 引入 `ShutdownHandle` 或 `CancellationToken`，与 Controller 相同模式
2. gRPC 改用 `serve_with_shutdown()`
3. HTTP 改用 `with_graceful_shutdown()`
4. 将 `tokio::select!` + 裸 `spawn` 改为 `tokio::join!` + shutdown signal

#### 问题 2：edgion-center 不响应 SIGTERM

**位置**：`src/core/center/cli/mod.rs:99`

**现象**：
- 仅监听 `tokio::signal::ctrl_c()` (SIGINT)
- **不监听 SIGTERM**

**影响**：
- 在 K8s 环境中，Pod 终止时 kubelet 发送 SIGTERM
- edgion-center 无法响应 SIGTERM，kubelet 需等待 `terminationGracePeriodSeconds`（默认 30s）后才 SIGKILL
- 等价于每次 Pod 滚动更新都要等 30s，严重影响部署效率

**建议修复**：
- 添加 SIGTERM 监听，与 Controller 的 `ShutdownHandle::wait_for_signals()` 一致

### 🟡 P1 — 中等问题

#### 问题 3：Controller Federation Client 未接入 ShutdownHandle

**位置**：`src/core/controller/cli/mod.rs:280-301`

**现象**：
```rust
let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
// ...
tokio::spawn(fed_client::run(..., shutdown_rx));
Some(shutdown_tx) // kept alive as _fed_shutdown_tx
```

`shutdown_tx` 是独立的 `watch::channel`，**不与 `ShutdownHandle` 关联**。收到 SIGINT/SIGTERM 时：
- `ShutdownHandle` 触发 → gRPC、Admin、ConfMgr 优雅关闭
- `shutdown_tx` 从未被 `send(true)` → Federation Client **永远收不到关闭信号**

**影响**：
- Federation Client 的 gRPC 双向 stream 不会主动关闭
- 进程退出时 stream 被强制中断，center 侧会看到连接异常
- 不影响数据一致性（client 是只读 + 幂等响应），但日志噪音大

**建议修复**：
```rust
// 方案 A：将 ShutdownHandle 的 signal 转接给 fed_client
let mut fed_signal = shutdown_handle.signal();
let (shutdown_tx, shutdown_rx) = watch::channel(false);
tokio::spawn(async move {
    fed_signal.wait().await;
    let _ = shutdown_tx.send(true);
});
tokio::spawn(fed_client::run(..., shutdown_rx));

// 方案 B：让 fed_client 直接接受 ShutdownSignal
```

#### 问题 4：Controller gRPC 对 streaming RPC 无关闭超时

**位置**：`src/core/controller/conf_sync/conf_server/grpc_server.rs:119-122`

**现象**：
- `serve_with_shutdown` 依赖 tonic 内部的 graceful shutdown
- tonic 会等待所有 in-flight streams 完成
- `watch` 和 `watch_server_meta` 是 server-streaming RPC，客户端不主动断开就会一直阻塞

**影响**：
- 如果 gateway 客户端因网络问题未感知到 GOAWAY，stream 可能永远不结束
- 进程关闭可能挂住

**缓解因素**：
- `watch` 流内的 `check_interval.tick()` 每秒检查一次 server_id 变化，但这与进程关闭无关
- 客户端断开后 `tx.send()` 会 `is_err()`，spawned 的流转发任务会自行退出
- 但 tonic 层面仍在等待 HTTP/2 stream 层面的关闭

**建议修复**：
```rust
// 在 serve_with_shutdown 外加超时包装
tokio::select! {
    result = grpc_server.serve_with_shutdown(...) => result,
    _ = tokio::time::sleep(Duration::from_secs(15)) => {
        tracing::warn!("gRPC graceful shutdown timed out, forcing stop");
        Ok(())
    }
}
```

#### 问题 5：Gateway 辅助 HTTP 服务无优雅关闭

**位置**：`src/core/gateway/cli/mod.rs:73-111`

**现象**：
- Admin API (5900) 和 Metrics API 在 `tokio::spawn` 中用裸 `axum::serve` 启动
- Pingora 的 `run_forever()` 返回后，main 函数结束
- 后台 tokio runtime 线程被操作系统终止，spawned 的 HTTP 服务没有优雅关闭机会

**影响**：
- Admin API 正在处理的请求可能被中断
- Metrics 拉取（Prometheus scrape）可能得到不完整响应
- 实际影响较小，因为这些都是短请求且非关键路径

**建议修复**：
- 将 Pingora 的关闭信号桥接到 tokio 任务
- 或使用 `CancellationToken` 协调 Pingora 和 Axum 服务的关闭

### 🟢 P2 — 改进建议

#### 问题 6：缺少全局关闭超时保护

**现象**：三个组件均无全局 "safety net" 超时。如果优雅关闭流程卡住，进程可能无限挂起。

**建议**：在 main 函数层面设置硬超时（如 60s），超时后 `std::process::exit(1)`。

#### 问题 7：关闭顺序未经设计

**现象**：Controller 中 gRPC 和 Admin HTTP 是并行关闭（`tokio::join!`）。虽然当前可行，但理想的关闭顺序应为：

1. **先关 Admin API**（停止接受新的管理操作）
2. **再关 gRPC**（让 gateway 客户端有时间完成最后一批同步）
3. **最后关 ConfMgr**（停止配置中心连接）

当前并行关闭不会导致 bug，但在高负载滚动更新时，可能出现 Admin API 已关但 gRPC 还在推送配置变更的短暂窗口（极端边界场景）。

---

## 五、对比表：各组件关闭能力矩阵

| 能力 | Controller | Center | Gateway (Pingora) | Gateway (Axum 辅助) |
|------|:----------:|:------:|:-----------------:|:-------------------:|
| SIGINT 处理 | ✅ | ✅ | ✅ (Pingora) | ❌ |
| SIGTERM 处理 | ✅ | ❌ | ✅ (Pingora) | ❌ |
| gRPC 优雅关闭 | ✅ | ❌ | N/A | N/A |
| HTTP 优雅关闭 | ✅ | ❌ | ✅ (Pingora数据面) | ❌ |
| 连接排空 | ✅ (hyper内部) | ❌ | ✅ (keepalive禁用 + Connection:close) | ❌ |
| 关闭超时 | ❌ | ❌ | ✅ (grace_period + timeout) | ❌ |
| 统一信号源 | ✅ (ShutdownHandle) | ❌ (不统一) | 部分 (仅Pingora) | ❌ |
| Federation Client 关闭 | ❌ (信号未接通) | N/A | N/A | N/A |

---

## 六、推荐修复优先级

| 优先级 | 问题 | 组件 | 工作量预估 |
|--------|------|------|-----------|
| P0 | Center gRPC/HTTP 无优雅关闭 | edgion-center | 2-3h |
| P0 | Center 不响应 SIGTERM | edgion-center | 30min |
| P1 | Controller Fed Client 未接入关闭信号 | edgion-controller | 30min |
| P1 | gRPC streaming 无关闭超时 | edgion-controller | 1h |
| P1 | Gateway 辅助 HTTP 无优雅关闭 | edgion-gateway | 1-2h |
| P2 | 全局关闭超时保护 | 全部 | 1h |
| P2 | 关闭顺序优化 | edgion-controller | 1h |

---

## 七、相关文件索引

| 文件 | 角色 |
|------|------|
| `src/core/controller/cli/mod.rs` | Controller 入口，信号 + 并行关闭 gRPC/Admin |
| `src/core/controller/conf_mgr/sync_runtime/shutdown.rs` | ShutdownHandle / ShutdownSignal / wait_for_signals |
| `src/core/controller/conf_sync/conf_server/grpc_server.rs` | tonic ConfigSync `serve_with_shutdown` |
| `src/core/controller/api/mod.rs` | axum Admin API `with_graceful_shutdown` |
| `src/core/controller/fed_sync/fed_client/mod.rs` | Federation Client（独立 watch channel） |
| `src/core/center/cli/mod.rs` | Center 入口，裸 serve + 仅 ctrl_c |
| `src/core/gateway/cli/mod.rs` | Gateway 入口，Pingora + tokio::spawn 辅助服务 |
| `src/core/gateway/cli/pingora.rs` | Pingora ServerConf (grace_period / shutdown_timeout) |
| `src/core/gateway/routes/http/proxy_http/pg_early_request_filter.rs` | Pingora 关闭时禁用 keepalive |
| `src/core/gateway/routes/http/proxy_http/pg_response_filter.rs` | Pingora 关闭时添加 Connection: close |
