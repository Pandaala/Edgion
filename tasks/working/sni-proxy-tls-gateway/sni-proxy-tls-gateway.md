# SNI Proxy / TLS Gateway 架构设计

> Edgion Gateway 的 TLS 终止 + SNI 路由功能，支持 Proxy Protocol v2、StreamPlugins 流控、结构化双事件日志。

## 1. 整体架构

```
Client (TLS)
  │
  │ TLS ClientHello (SNI: *.sandbox.com)
  ▼
┌───────────────────────────────────────────────┐
│ Pingora TLS Listener (port 31280)             │
│  ├─ TLS Terminate (via EdgionTls cert)        │
│  └─ Extract SNI from ssl_ref                  │
│     │                                         │
│     ▼                                         │
│  GatewayTlsRoutes.match_route(sni)            │
│     │                                         │
│     ├─ No match → close connection            │
│     │                                         │
│     ▼ Matched TLSRoute                        │
│  StreamPlugins (optional, from annotation)    │
│     │                                         │
│     ├─ Deny → close connection                │
│     │                                         │
│     ▼ Allow                                   │
│  select_roundrobin_backend(service_key)        │
│     │                                         │
│     ▼                                         │
│  TCP connect to upstream                      │
│     │                                         │
│     ├─ PP2 header (optional, from annotation) │
│     │   └─ AUTHORITY TLV = SNI hostname       │
│     │                                         │
│     ▼                                         │
│  Bidirectional duplex forwarding              │
│     │                                         │
│     ▼                                         │
│  Log: connect event + disconnect event        │
└───────────────────────────────────────────────┘
  │
  ▼
Backend (TCP echo / upstream service)
```

## 2. 核心组件

### 2.1 EdgionTls (`src/core/gateway/routes/tls/edgion_tls.rs`)

实现 Pingora 的 `ServerApp` trait，是 TLS 数据面的主入口。

**连接处理流程 (`process_new`)：**

1. 提取客户端地址信息
2. 从 TLS SSL 层提取 SNI hostname
3. 基于 SNI 匹配 TLSRoute
4. 执行 StreamPlugins（如有配置）
5. 选择后端（WeightedRoundRobin）
6. TCP 连接上游
7. 发送 PP2 header（如有配置）
8. 双向数据转发 (`duplex`)
9. 记录 connect/disconnect 日志

**关键数据结构：**

- `TlsContext`：单连接生命周期的上下文，携带地址、时间、字节计数、状态
- `TlsStatus`：连接结果枚举（Success/NoSniProvided/DeniedByPlugin 等）

### 2.2 Proxy Protocol v2 编码器 (`src/core/common/utils/proxy_protocol.rs`)

完整实现 HAProxy PP2 二进制协议：

- 12 字节签名 + 4 字节头 + 地址块 + TLV 链
- 支持 IPv4 (12 bytes)、IPv6 (36 bytes)、混合地址族（v4→mapped v6）
- TLV 扩展：AUTHORITY (0x02) 携带 SNI hostname、ALPN (0x01) 等
- Builder 模式：`ProxyProtocolV2Builder::new(src, dst).add_authority(sni).build()`

### 2.3 TLS 日志模块 (`src/core/gateway/observe/logs/tls_log.rs`)

结构化双事件日志：

| 字段 | connect | disconnect |
|------|---------|------------|
| ts | 连接时间 | 断开时间 |
| event | "connect" | "disconnect" |
| duration_ms | - | 有 |
| bytes_sent/received | - | 有 |
| status | 当前状态 | 最终状态 |
| proxy_protocol | "v2" / null | "v2" / null |
| route_name | ns/name | ns/name |

全局单例 logger，通过 `OnceLock<Arc<AccessLogger>>` 管理，在 CLI 启动时初始化。

### 2.4 TLSRoute 资源扩展 (`src/types/resources/tls_route.rs`)

`TLSRouteRule` 新增三个 runtime-only 字段（`#[serde(skip)]`）：

| 字段 | 类型 | 来源 |
|------|------|------|
| `proxy_protocol_version` | `Option<u8>` | annotation `edgion.io/proxy-protocol: "v2"` |
| `upstream_tls` | `bool` | annotation `edgion.io/upstream-tls: "true"` |
| `stream_plugin_store_key` | `Option<String>` | annotation `edgion.io/edgion-stream-plugins: "ns/name"` |

### 2.5 配置处理 (`src/core/gateway/routes/tls/conf_handler_impl.rs`)

`TlsRouteManager::initialize_route()` 在路由加载时：

1. 解析 annotations → 填充 runtime 字段
2. 初始化 `BackendSelector`（加权轮询）
3. 解析 stream plugin store key（支持短名自动补 namespace）
4. 支持 `full_set` 和 `partial_update` 两种更新模式

## 3. Annotation 配置参考

| Annotation | 值 | 作用 | 示例 |
|------------|-----|------|------|
| `edgion.io/proxy-protocol` | `"v2"` | 向上游发送 PP2 header | PP2 with AUTHORITY TLV |
| `edgion.io/upstream-tls` | `"true"/"false"` | 上游 TLS（尚未实现） | - |
| `edgion.io/edgion-stream-plugins` | `"ns/name"` 或 `"name"` | 关联 StreamPlugin 资源 | IP 限制/速率限制 |
| `edgion.io/backend-protocol` | `"tcp"` | Gateway 级后端协议 | 设置在 Gateway 上 |

## 4. 注意事项

### 4.1 SNI 提取依赖 TLS feature

`extract_sni()` 仅在 `feature = "boringssl"` 或 `feature = "openssl"` 时有效。如果编译时未启用这些 feature，SNI 始终返回 `None`，所有 TLS 连接都会被拒绝。

### 4.2 PP2 混合地址族处理

当 src 和 dst 地址族不同时（一个 IPv4 一个 IPv6），PP2 编码器会将 IPv4 地址映射为 IPv4-mapped IPv6 (::ffff:x.x.x.x)，使用 AF_INET6 格式。这符合 PP2 规范要求。

### 4.3 StreamPlugin 执行时机

StreamPlugin 在 TLS 握手**之后**、后端连接**之前**执行。这意味着：
- 客户端 IP 已知（可做 IP 限制）
- TLS 资源已消耗（恶意客户端可消耗 TLS 握手资源）
- 与 Gateway 级 ConnectionFilter（在 TLS 之前）形成互补

### 4.4 日志双写

`log_disconnect` 同时写入：
- TLS 专用 logger（结构化 JSON，可独立配置输出路径）
- per-listener access_logger（与 HTTP/TCP 日志统一）

### 4.5 PP2 集成测试

test_server 提供了一个 PP2-aware TCP 服务器 (`--tcp-pp2-port`)，使用 `proxy-header` crate 解析 PP2 header，返回结构化 JSON 响应：

```json
{
  "pp2": true,
  "src_addr": "127.0.0.1:52341",
  "dst_addr": "127.0.0.1:30012",
  "authority": "test-443.pp2.example.com",
  "peer_addr": "127.0.0.1:xxxxx",
  "pp2_header_len": 54
}
```

测试客户端通过验证 `pp2=true`、`authority` 字段匹配 SNI hostname、`src_addr` 非空来断言 PP2 header 被正确发送和解析。

PP2 测试使用独立的 Gateway listener（port 31281, hostname `*.pp2.example.com`）和独立的后端 Service `test-tcp-pp2:30012`，避免与普通 TLS 路由混淆。

## 5. 已完成

- [x] Phase 1: Proxy Protocol v2 编码器 + 完整单元测试
- [x] Phase 2: TLS 日志模块（双事件 connect/disconnect）
- [x] Phase 3: TLSRoute 资源扩展（3 个 runtime 字段）
- [x] Phase 4: conf_handler annotation 解析 + 单元测试
- [x] Phase 5: EdgionTls 核心逻辑增强（PP2/StreamPlugins/日志集成）
- [x] Phase 6: listener_builder 检查（确认无需改动）
- [x] Phase 7: 集成测试套件（Basic/PP2/StreamPlugins）
- [x] PP2 test_server 解析：使用 `proxy-header` crate，返回结构化 JSON
- [x] PP2 配置 hostname 修复：Gateway 增加独立 `tls-pp2` listener (port 31281)
- [x] K8s 集成测试配置：`examples/k8stest/conf/TLSRoute/` 完整配置（无 EndpointSlice）
- [x] Clippy 警告修复：`edgion_tls.rs` 中的 `is_err()` 和多余引用

## 6. 待完成

- [ ] **上游 TLS (P1)**：`upstream_tls` 字段已预留，annotation 解析已实现，但实际的 upstream TLS 连接逻辑未实现（需要 `tokio-rustls` 客户端）
- [ ] **Proxy Protocol v1 支持**：如有需求，扩展编码器和 annotation 值支持 `"v1"`
- [ ] **连接超时配置**：上游连接超时目前使用 Pingora 默认值，可通过 annotation 暴露
- [ ] **metrics 集成**：TLS 连接的 Prometheus 指标（连接数、延迟、PP2 发送量等）

## 7. 文件清单

| 文件 | 状态 | 说明 |
|------|------|------|
| `src/core/common/utils/proxy_protocol.rs` | 新建 | PP2 编码器 |
| `src/core/common/utils/mod.rs` | 修改 | 导出 proxy_protocol 模块 |
| `src/core/gateway/observe/logs/tls_log.rs` | 新建 | TLS 日志模块 |
| `src/core/gateway/observe/logs/mod.rs` | 修改 | 导出 tls_log |
| `src/core/gateway/observe/mod.rs` | 修改 | 再导出 TLS 日志组件 |
| `src/core/gateway/cli/config.rs` | 修改 | 添加 tls_log 配置 |
| `src/core/gateway/cli/mod.rs` | 修改 | 初始化 TLS logger |
| `src/types/resources/tls_route.rs` | 修改 | TLSRouteRule 新增 3 个字段 |
| `src/types/constants/annotations.rs` | 修改 | 新增 PP2/upstream-tls 常量 |
| `src/core/gateway/routes/tls/conf_handler_impl.rs` | 修改 | annotation 解析逻辑 |
| `src/core/gateway/routes/tls/edgion_tls.rs` | 修改 | 核心 TLS 处理逻辑 |
| `examples/code/server/test_server.rs` | 修改 | 新增 PP2-aware TCP 服务器 |
| `examples/code/client/suites/tls_route/**` | 新建 | 集成测试套件 |
| `examples/test/conf/TLSRoute/**` | 新建/修改 | 本地测试配置 |
| `examples/k8stest/conf/TLSRoute/**` | 新建 | K8s 测试配置 |
| `examples/test/scripts/utils/start_all_with_conf.sh` | 修改 | 添加 `--tcp-pp2-port 30012` |
