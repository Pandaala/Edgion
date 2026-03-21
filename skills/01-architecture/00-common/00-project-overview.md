---
name: project-overview
description: Edgion 项目高层架构图、Crate 结构、三个 bin 定义、代码组织、EdgionHttpContext、测试基础设施、关键依赖。
---

# 项目总览

## 高层架构

```
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-controller                       │
                    │                                                          │
  YAML/K8s CRD ──► │  ConfCenter ──► Workqueue ──► ResourceProcessor          │
                    │  (File/K8s)     (per-kind)    (validate/preparse/parse)  │
                    │                                                          │
  edgion-ctl ────► │  Admin API (:5800)   ConfigSyncServer (gRPC :50051)      │
                    └─────────────────────────────┬────────────────────────────┘
                                                  │ gRPC Watch/List
                                                  ▼
                    ┌──────────────────────────────────────────────────────────┐
                    │                  edgion-gateway                          │
                    │                                                          │
                    │  ConfigSyncClient ──► ClientCache ──► Preparse           │
                    │                       (per-kind)                         │
                    │  Pingora Server                                          │
                    │  ├─ ConnectionFilter (TCP-level, StreamPlugins)          │
                    │  ├─ ProxyHttp (HTTP/gRPC lifecycle)                      │
                    │  │  ├─ request_filter     → route match + plugins        │
                    │  │  ├─ upstream_peer      → backend selection + LB       │
                    │  │  ├─ upstream_response  → response plugins             │
                    │  │  └─ logging            → AccessLog                    │
                    │  └─ TCP/UDP/TLS Routes                                   │
                    │                                                          │
                    │  Admin API (:5900)   Metrics API (:5901)                 │
                    └──────────────────────────────────────────────────────────┘
```

## Crate 结构

单 Crate（非 workspace），三个 `[[bin]]` 目标：

| Binary | 入口文件 | 运行时模型 | 角色 |
|--------|---------|-----------|------|
| `edgion-gateway` | `src/bin/edgion_gateway.rs` | 同步入口，内部自管理 Tokio + Pingora 主循环 | 数据面 |
| `edgion-controller` | `src/bin/edgion_controller.rs` | `#[tokio::main(multi_thread)]` | 控制面 |
| `edgion-ctl` | `src/bin/edgion_ctl.rs` | `#[tokio::main]` 单线程 | CLI 工具 |

示例二进制（测试用）：`test_server`、`test_client`、`test_client_direct`、`resource_diff`、`config_load_validator`。

默认 features：`allocator-jemalloc` + `boringssl`。

## 代码目录组织

```
src/
├── bin/                         # 薄入口包装（thin wrappers）
│   ├── edgion_gateway.rs        #   → EdgionGatewayCli::run()
│   ├── edgion_controller.rs     #   → EdgionControllerCli::run()
│   └── edgion_ctl.rs            #   → Cli::run()
├── lib.rs                       # Crate root: pub mod core, pub mod types
├── core/                        # 所有业务逻辑
│   ├── controller/              # edgion-controller 归属代码
│   │   ├── api/                 #   Admin API
│   │   ├── cli/                 #   CLI 入口和启动
│   │   ├── conf_mgr/            #   配置中心 / Workqueue / ResourceProcessor
│   │   ├── conf_sync/           #   gRPC server + ServerCache
│   │   ├── observe/             #   日志 facade
│   │   └── services/            #   ACME 等服务
│   ├── gateway/                 # edgion-gateway 归属代码
│   │   ├── api/                 #   Admin API
│   │   ├── backends/            #   后端发现 / 健康检查 / BackendTLSPolicy
│   │   ├── cli/                 #   CLI 入口和 Pingora 启动
│   │   ├── config/              #   GatewayClass / EdgionGatewayConfig
│   │   ├── conf_sync/           #   gRPC client + ClientCache
│   │   ├── lb/                  #   负载均衡算法
│   │   ├── link_sys/            #   外部系统（Redis/Etcd/ES/Webhook/File）
│   │   ├── observe/             #   AccessLog / Metrics / 协议日志
│   │   ├── plugins/             #   插件系统（http / stream / runtime）
│   │   ├── routes/              #   HTTP / gRPC / TCP / TLS / UDP 路由
│   │   ├── runtime/             #   Pingora 运行时（server / matching / store）
│   │   ├── services/            #   ACME challenge 服务
│   │   └── tls/                 #   TLS 运行时 / 证书存储 / 验证
│   ├── ctl/                     # edgion-ctl 归属代码
│   │   └── cli/                 #   commands / output / client
│   └── common/                  # 跨 bin 共享模块
│       ├── config/              #   共享启动配置（test mode, cache config）
│       ├── conf_sync/           #   gRPC proto / traits / 共享类型
│       ├── matcher/             #   域名匹配、IP Radix 树
│       └── utils/               #   metadata / net / duration / real_ip 等工具
└── types/                       # 纯数据定义（无业务逻辑）
    ├── resource/                # 资源系统（define_resources!, ResourceKind, ResourceMeta）
    ├── resources/               # 各类资源结构体（Gateway, HTTPRoute, EdgionPlugins, ...）
    ├── common/                  # KeyGet/KeySet 统一访问器
    ├── constants/               # 注解、标签、头部、Secret 键
    ├── ctx.rs                   # EdgionHttpContext（每请求状态）
    ├── filters.rs               # PluginRunningResult, PluginRunningStage
    ├── schema.rs                # JSON Schema 验证
    └── err.rs                   # 错误类型
```

**设计原则**：`types/` 只放纯数据定义，`core/` 放所有逻辑。`core/` 按二进制归属分层（controller / gateway / ctl / common），不使用顶层 shim 模块。

## EdgionHttpContext — 每请求状态载体

定义在 `src/types/ctx.rs`，贯穿整个 HTTP 请求生命周期的"背包"：

| 字段 | 用途 |
|------|------|
| `start_time` | 请求计时 |
| `gateway_info` | Gateway 元数据 |
| `request_info` | 客户端地址、hostname、path、trace ID、SNI、gRPC 元数据 |
| `edgion_status` | 处理过程中累积的错误码 |
| `route_unit` / `grpc_route_unit` | 匹配到的路由规则（内含 `PluginRuntime`） |
| `selected_backend` / `selected_grpc_backend` | 选中的后端引用 |
| `backend_context` | Service 名称、上游尝试次数、连接时间 |
| `stage_logs` | `Vec<StageLogs>` — 每执行阶段的插件日志 |
| `plugin_running_result` | 当前插件链执行结果 |
| `ctx_map` | `HashMap<String, String>` — 插件设置的变量 |
| `path_params` | 路由路径参数（懒提取） |
| `hash_key` | 一致性哈希键 |
| `try_cnt` | 上游连接尝试计数器 |

在 `new_ctx()` 中创建，在 `logging()` 中消费。插件通过 `PluginSession` 适配器与之交互。

## 测试基础设施

| 组件 | 路径 | 用途 |
|------|------|------|
| `test_server` | `examples/code/server/test_server.rs` | 多协议回显后端（HTTP、gRPC、WebSocket、TCP、UDP、auth） |
| `test_client` | `examples/code/client/test_client.rs` | 基于 TestSuite trait 的测试运行器 |
| `resource_diff` | `examples/code/validator/resource_diff.rs` | Controller ↔ Gateway 同步验证 |
| `run_integration.sh` | `examples/test/scripts/integration/` | 完整集成测试编排 |
| 测试配置 | `examples/test/conf/` | 按 `Resource/Item/` 组织的 YAML 资源 |
| 端口注册 | `examples/test/conf/ports.json` | 每个测试套件的唯一端口分配 |

## 关键依赖

| 类别 | Crate | 用途 |
|------|-------|------|
| **代理核心** | `pingora-core`, `pingora-proxy`, `pingora-http`, `pingora-load-balancing` | HTTP 代理引擎 |
| **异步** | `tokio`, `tokio-stream`, `futures`, `async-trait` | 异步运行时 |
| **gRPC** | `tonic`, `tonic-reflection`, `prost` | Controller ↔ Gateway 通信 |
| **HTTP API** | `axum`, `tower-http`, `hyper-util` | Admin API |
| **K8s** | `kube`, `k8s-openapi`, `schemars` | K8s 集成 + CRD Schema |
| **序列化** | `serde`, `serde_json`, `serde_yaml`, `toml` | 配置解析 |
| **TLS** | `rustls`, `tokio-rustls`, `boring-sys` | TLS 终止（rustls 或 BoringSSL） |
| **可观测** | `tracing`, `metrics` | 日志 + 指标 |
| **安全** | `jsonwebtoken`, `bcrypt`, `base64` | 认证插件 |
| **性能** | `tikv-jemallocator`, `dashmap`, `arc-swap`, `smallvec` | 内存分配器、并发 Map、无锁读、栈缓冲 |
