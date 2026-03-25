---
name: gateway-startup-shutdown
description: edgion-gateway 启动与关闭流程：14 步启动序列、两阶段 Pingora 集成、Tokio 运行时生命周期管理。
---

# Gateway 启动与关闭

## 启动入口

`EdgionGatewayCli::run()` (`cli/mod.rs`) 编排整个启动流程。该方法在 `main()` 线程上同步执行，内部创建 Tokio Runtime 用于异步操作，最终将控制权交给 Pingora 主循环。

## 14 步启动序列

```rust
// cli/mod.rs - EdgionGatewayCli::run()
pub fn run(&self) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    // ... 14 steps below ...
}
```

### 第 1 步：加载 TOML 配置

```rust
let config = EdgionGatewayConfig::load(self.config.clone())?;
```

`EdgionGatewayConfig::load()` 合并 CLI 参数与 TOML 配置文件。配置结构定义在 `cli/config.rs`，包含 server、access_log、ssl_log、tcp_log、tls_log、udp_log、rate_limit、all_endpoint_status 等段。

### 第 2 步：初始化工作目录

```rust
init_work_dir(work_dir_path)?;
let wd = work_dir();
wd.validate()?;
```

工作目录优先级：CLI `--work-dir` > 环境变量 `EDGION_WORK_DIR` > 配置文件 `work_dir` > 当前目录 `"."`。`init_work_dir()` 创建必要的子目录结构并通过 `validate()` 校验可写性。

### 第 3 步：初始化日志系统

```rust
let log_config = config.to_log_config();
let _log_guard = runtime.block_on(init_logging(log_config))?;
```

`init_logging()` 配置 tracing 日志框架，返回 `WorkerGuard` 以确保日志在进程退出时刷写。同时初始化 RateLimit 全局配置和 AllEndpointStatus 全局配置。

### 第 4 步：创建 ConfigSyncClient（gRPC 到 Controller）

```rust
let mut sync_client = runtime.block_on(Self::create_config_sync_client(&config))?;
let config_client = sync_client.get_config_client();
```

`ConfigSyncClient::new()` 建立到 Controller 的 gRPC 连接，参数包含 `server_addr`、客户端名称 `"edgion-gateway"` 和 10 秒超时。`get_config_client()` 返回 `Arc<ConfigClient>`，聚合所有资源类型的 ClientCache。

### 第 5 步：初始化全局 ConfigClient

```rust
init_global_config_client(config_client.clone())?;
```

将 ConfigClient 注册到全局 `OnceLock<Arc<ConfigClient>>`，使其他模块可通过 `get_global_config_client()` 访问。

### 第 6 步：从 Controller 获取 ServerInfo

```rust
let server_info = runtime.block_on(sync_client.get_server_info())?;
```

`GetServerInfo` gRPC 调用返回：
- `endpoint_mode`：`EndpointSlice` / `Endpoint` / `Both`，决定后端发现模式
- `supported_kinds`：Controller 支持的资源类型列表，用于选择性 Watch
- `server_id`：Controller 实例标识，用于版本一致性校验

解析 endpoint_mode 并通过 `init_global_endpoint_mode()` 设置全局模式。同时初始化集成测试模式和全局测试模式（由 `--integration-testing-mode` CLI 标志控制）。

### 第 7 步：开始 Watch 资源

```rust
runtime.block_on(sync_client.start_watch_kinds(&server_info.supported_kinds))?;
```

基于 Controller 返回的 `supported_kinds` 列表，为每种资源类型启动 gRPC Watch 流。同时启动 `start_watch_server_meta()` 后台任务，监控 gateway 实例数量用于 Cluster 级别限流。

### 第 8 步：启动辅助服务

```rust
runtime.block_on(Self::start_auxiliary_services(config_client.clone()));
```

`start_auxiliary_services()` 启动四个组件：
1. **BackendCleaner**：`BackendCleaner::new().start()`，清理 LeastConnection LB 的过期后端状态
2. **Health Check Manager**：`get_hc_config_store()` + `get_health_check_manager()` 初始化健康检查配置存储和管理器
3. **Admin API**：`tokio::spawn(api::serve(config_client, 5900))`，端口 5900
4. **Metrics API**：`tokio::spawn(metrics::serve(5901))`，端口 5901，Prometheus 指标暴露

### 第 9 步：等待所有缓存就绪

```rust
runtime.block_on(Self::wait_for_ready(config_client.clone()))?;
```

轮询 `config_client.is_ready()`（每秒一次），直到所有 ClientCache 完成首次 List（`CacheData.is_ready() == true`）。这确保 Gateway/GatewayClass/EdgionGatewayConfig/HTTPRoute 等资源在开始接收流量前已加载。

### 第 10 步：预加载负载均衡器

```rust
crate::core::gateway::backends::preload_load_balancers(config_client.clone());
```

遍历所有已加载的路由，为每个 BackendRef 预热 LB 选择器（后端发现 + 权重计算）。减少首请求延迟。

### 第 11 步：初始化所有可观测性 Logger

```rust
runtime.block_on(init_access_logger(&config.access_log))?;
runtime.block_on(init_ssl_logger(&config.ssl_log))?;
runtime.block_on(init_tcp_logger(&config.tcp_log))?;
runtime.block_on(init_tls_logger(&config.tls_log))?;
runtime.block_on(init_udp_logger(&config.udp_log))?;
```

五个 Logger 分别处理 HTTP 访问日志、SSL 握手日志、TCP 连接日志、TLS 路由日志和 UDP 日志。每个 Logger 独立配置输出目标（文件/stdout/LinkSys）。

### 第 12 步：创建 Pingora 服务器（Phase 1 — 配置 Listener）

```rust
let pingora_server = runtime.block_on(async {
    tokio::task::spawn_blocking(move || create_and_configure_server(config_client, &config))
        .await
        .expect("Failed to spawn blocking task")
})?;
```

Phase 1 在 Tokio Runtime 上下文中执行（`spawn_blocking` 确保 UDP listener 可用 Tokio UDP socket）：

1. **创建 ServerConf**：从 TOML 配置读取 `threads`（默认 CPU 核数）、`work_stealing`（默认 true）、`grace_period_seconds`（默认 30）、`graceful_shutdown_timeout_seconds`（默认 10）、`upstream_keepalive_pool_size`（默认 128）
2. **Bootstrap**：`Server::new_with_opt_and_conf(None, server_conf)` + `pingora_server.bootstrap()`
3. **创建 GatewayBase**：`GatewayBase::new(config_client)`
4. **获取 Gateway 列表**：`config_client.list_gateways().data`
5. **配置 Listener**：`gateway_base.configure_listeners(&mut pingora_server, gateways)`

`configure_listeners()` 遍历每个 Gateway 的每个 Listener，依次：
- 查找关联的 GatewayClass 和 EdgionGatewayConfig
- 检查端口冲突和 Conflicted 状态
- 解析 HTTP/2 annotation (`edgion.io/enable-http2`)
- 构建 `ListenerContext` 并调用 `listener_builder::add_listener()` 将 Listener 添加到 Pingora Server

### 第 13 步：将 Tokio 运行时移到后台线程

```rust
std::thread::spawn(move || {
    runtime.block_on(async {
        std::future::pending::<()>().await;
    });
});
```

将 Tokio Runtime 所有权移入后台线程。`std::future::pending()` 使线程永远不退出，保持所有 Tokio 异步任务（gRPC Watch、Admin API、Metrics API、Health Check 等）持续运行。

### 第 14 步：运行 Pingora 服务器（Phase 2 — 阻塞主循环）

```rust
run_server(pingora_server);
// fn run_server(server: Server) { server.run_forever(); }
```

Phase 2 在主线程上调用 `server.run_forever()`，启动 Pingora 工作线程池接收连接并处理请求。此调用阻塞直到收到关闭信号。

## 两阶段 Pingora 集成

| 阶段 | 函数 | 运行线程 | 职责 |
|------|------|---------|------|
| Phase 1 | `create_and_configure_server()` | Tokio Runtime (spawn_blocking) | 创建 ServerConf、Bootstrap、配置所有 Listener |
| Phase 2 | `run_server()` → `server.run_forever()` | 主线程 | 启动 Pingora 工作线程池，开始接收连接 |

分离原因：Phase 1 需要 Tokio 上下文（UDP listener 依赖 Tokio UDP socket），而 Phase 2 的 `run_forever()` 会接管线程控制权，所以必须在 Tokio Runtime 被移走之后调用。

## 关闭流程

关闭依赖 Pingora 内置的优雅关闭机制：

1. Pingora 收到 SIGTERM/SIGINT 信号
2. 进入 grace period（默认 30 秒）：现有连接继续服务，`session.is_process_shutting_down()` 返回 true
3. 在 `early_request_filter` 中禁用 keepalive：`session.set_keepalive(None)`
4. 在 `response_filter` 中添加 `Connection: close` 响应头
5. grace period 结束后，进入 graceful shutdown timeout（默认 10 秒）强制关闭剩余连接
6. `run_forever()` 返回，主线程退出
7. 后台 Tokio 线程随进程退出自动终止
