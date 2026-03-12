---
name: config-reference
description: Centralized configuration reference for TOML config files and EdgionGatewayConfig CRD.
---
# Configuration Reference

> Centralized configuration reference for TOML config files and EdgionGatewayConfig CRD.
>
> **TODO (2026-02-25): P2, New**
> - [ ] Controller TOML parameter table (`server`, `conf_center`, `logging`, `conf_sync`, `debug`, etc.)
> - [ ] Gateway TOML parameter table (`gateway`, `logging`, `access_log`, `ssl_log`, `tcp_log`, `udp_log`, `pingora_server`, etc.)
> - [ ] Two `conf_center` modes comparison (file_system vs kubernetes)
> - [ ] CLI args vs TOML override relationship
> - [ ] K8s mode specific config (`gateway_class`, `watch_namespaces`, leader election)

## EdgionGatewayConfig CRD — ServerConfig

CRD: `edgion.io/v1alpha1 EdgionGatewayConfig`, referenced via GatewayClass `parametersRef`.

Source: `src/types/resources/edgion_gateway_config.rs`

### spec.server

| YAML 字段 | Rust 类型 | 默认值 | 说明 |
|-----------|-----------|--------|------|
| `threads` | `Option<u32>` | CPU 核数 | Pingora worker 线程数 |
| `workStealing` | `bool` | `true` | tokio work-stealing 调度 |
| `gracePeriodSeconds` | `Option<u64>` | `30` | 优雅关闭宽限期（秒） |
| `gracefulShutdownTimeoutS` | `Option<u64>` | `10` | 优雅关闭超时（秒） |
| `upstreamKeepalivePoolSize` | `Option<u32>` | `128` | 上游 keepalive 连接池大小 |
| `enableCompression` | `bool` | `false` | 下游响应压缩 |
| `downstreamKeepaliveRequestLimit` | `u32` | `1000` | 下游连接复用请求数上限（见下） |

### downstreamKeepaliveRequestLimit 详解

等价于 Nginx 的 [`keepalive_requests`](https://nginx.org/en/docs/http/ngx_http_core_module.html#keepalive_requests)。

**行为**：限制单个下游 TCP 连接可以服务的最大 HTTP 请求数。达到上限后关闭连接，客户端需重新建连。

**作用范围**：
- **Per-connection**：每个 TCP 连接有独立计数器，非全局也非 per-worker
- **仅 HTTP/1.1**：HTTP/2 多路复用不受此限制（H2 的流数量由 H2 协议本身控制）
- 与 Nginx `keepalive_requests` 语义完全一致（Nginx 也是 per-connection）

**默认值 1000** 与 Nginx 一致。设为 `0` 禁用限制（Pingora 原始默认行为）。

**收益**：
- 改善负载均衡分布（防止连接"粘"在特定实例上）
- 释放 per-connection 内存分配
- 降低长连接被劫持风险

**Pingora API**：`HttpServerOptions::keepalive_request_limit: Option<u32>`，在 `ProxyServiceBuilder::server_options()` 设置。

**代码路径**：`listener_builder.rs` → `ProxyServiceBuilder::server_options(HttpServerOptions { keepalive_request_limit: Some(n), .. })`
