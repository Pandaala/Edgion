# TLS Gateway 数据面详解

> EdgionTls 的完整连接生命周期：从 TLS ClientHello 到双向转发，包括 SNI 路由、StreamPlugin 执行、Proxy Protocol v2 编码、结构化日志。

## 1. 数据面总览

TLS Gateway 数据面由 `EdgionTls` 实现 Pingora 的 `ServerApp` trait，每个 TLS listener 一个实例。它接收已完成 TLS 握手的连接，提取 SNI，匹配 `TLSRoute`，执行策略，连接上游，转发数据。

```
                  ┌──────────────────────────────┐
                  │       Pingora Runtime         │
                  │                              │
 Client ──TLS──► │  TLS Listener (port N)        │
                  │   └─ TLS Terminate (cert)     │
                  │       └─ EdgionTls::process_new()
                  │           │                   │
                  │           ▼                   │
                  │  extract_sni() ─── None ──► drop + log
                  │           │                   │
                  │         Some(sni)             │
                  │           ▼                   │
                  │  handle_connection()          │
                  │   ├─ match_route(sni)         │
                  │   ├─ stream_plugins()         │
                  │   ├─ select_backend()         │
                  │   ├─ connect_upstream()       │
                  │   ├─ send_pp2_header()        │
                  │   ├─ log_connect()            │
                  │   ├─ duplex() ◄──► upstream   │
                  │   └─ log_disconnect()         │
                  └──────────────────────────────┘
```

## 2. 连接处理流程

### 2.1 入口：`process_new`

`ServerApp::process_new` 是每个新 TLS 连接的入口。在 Pingora 完成 TLS 握手后调用。

**步骤：**

1. **Shutdown 检查**：如果 Gateway 正在 shutdown，直接拒绝新连接。
2. **提取客户端信息**：从 `Stream::get_socket_digest()` 获取 peer address（IP + port）。
3. **初始化 TlsContext**：创建连接级上下文，贯穿整个连接生命周期。
4. **提取 SNI**：调用 `extract_sni()`，失败则设置 `TlsStatus::NoSniProvided` 并断开。
5. **处理连接**：调用 `handle_connection()` 执行核心逻辑。
6. **记录断连日志**：无论成功失败，最终都调用 `log_disconnect()`。

### 2.2 SNI 提取

```rust
fn extract_sni(stream: &mut Stream) -> Option<String>
```

通过 `stream.get_ssl().servername(NameType::HOST_NAME)` 从 TLS 层获取 SNI。

**关键约束**：仅在编译时启用 `feature = "boringssl"` 或 `feature = "openssl"` 时有效。如果未启用，SNI 始终为 `None`，所有 TLS 连接都会被拒绝（`NoSniProvided`）。

### 2.3 SNI 路由匹配

```rust
gateway_tls_routes.match_route(sni_hostname) -> Option<Arc<TLSRoute>>
```

`GatewayTlsRoutes` 使用 `ArcSwap<HashMap<String, Vec<Arc<TLSRoute>>>>` 存储路由表，支持 lock-free 并发读取。

**匹配顺序：**

1. **精确匹配**：`sni_hostname` 直接查找 HashMap
2. **通配符匹配**：提取 `*.suffix` 形式（如 `test.example.com` → `*.example.com`）再查找

每个 Gateway 实例持有独立的 `GatewayTlsRoutes`，通过 `TlsRouteManager` 按 `namespace/gateway_name` 隔离。

### 2.4 StreamPlugin 执行

StreamPlugin 在 TLS 握手**之后**、上游连接**之前**执行。与 `EdgionTcp` 使用相同模式。

**执行流程：**

1. 检查 `rule.stream_plugin_store_key` 是否存在
2. 解析 client IP（`ctx.client_addr.parse()`）
3. 从全局 `StreamPluginStore` 获取 `EdgionStreamPlugins` 资源
4. 构造 `StreamContext`（client_ip, listener_port）
5. 执行 `runtime.run(&stream_ctx)`
6. `Allow` → 继续；`Deny(reason)` → 设置 `DeniedByPlugin` 状态并断开

**store_key 解析**（在 `conf_handler_impl.rs` 中）：

- 全路径格式：`edgion-test/my-plugins` → 直接使用
- 短名格式：`my-plugins` → 自动补 TLSRoute 的 namespace 前缀

**与 Gateway 级 ConnectionFilter 的区别**：

| 特性 | ConnectionFilter | StreamPlugin (TLSRoute) |
|------|-----------------|------------------------|
| 执行时机 | TLS 握手之前 | TLS 握手之后 |
| 粒度 | Gateway/Listener 级 | Route 级 |
| 可用信息 | 仅 IP | IP + SNI + route context |
| 资源消耗 | 低（未做 TLS 握手） | 较高（已消耗 TLS 握手资源） |

### 2.5 后端选择

两步过程：

1. **BackendRef 选择**（`rule.backend_finder.select()`）：从 TLSRoute 的 `backendRefs` 中按权重（Weighted Round-Robin）选择一个 service。
2. **Endpoint 解析**（`select_roundrobin_backend(&service_key)`）：从 EndpointSlice 中按 Round-Robin 选择具体的后端 Pod 地址。

```
TLSRoute.rules[0].backendRefs:     ← BackendSelector (权重轮询)
  - name: svc-a, weight: 80
  - name: svc-b, weight: 20
        │
        ▼
EndpointSlice (svc-a):              ← select_roundrobin_backend (轮询)
  - 10.244.1.10:8080
  - 10.244.1.11:8080
```

### 2.6 上游连接

通过 Pingora 的 `TransportConnector` 建立 TCP 连接：

```rust
let peer = BasicPeer::new(&upstream_addr_str);
let upstream = self.connector.new_stream(&peer).await?;
```

当前仅支持 TCP 上游。`upstream_tls` 字段已预留，annotation 解析已实现，但实际的 TLS 客户端连接逻辑尚未实现。

### 2.7 Proxy Protocol v2 编码

当 TLSRoute 配置了 `edgion.io/proxy-protocol: "v2"` 时，在上游连接建立后、数据转发前，发送 PP2 header。

**PP2 二进制格式：**

```
┌─────────────────────────────────────────────────┐
│ Signature (12 bytes)                             │
│ 0x0D 0x0A 0x0D 0x0A 0x00 0x0D 0x0A 0x51        │
│ 0x55 0x49 0x54 0x0A                             │
├─────────────────────────────────────────────────┤
│ Version+Command (1 byte): 0x21 (v2 + PROXY)    │
│ Family+Protocol (1 byte): 0x11 (IPv4+TCP)       │
│                            0x21 (IPv6+TCP)       │
│ Payload Length (2 bytes, big-endian)             │
├─────────────────────────────────────────────────┤
│ Address Block:                                   │
│   IPv4: src_ip(4) + dst_ip(4) + src_port(2)    │
│          + dst_port(2) = 12 bytes               │
│   IPv6: src_ip(16) + dst_ip(16) + src_port(2)  │
│          + dst_port(2) = 36 bytes               │
├─────────────────────────────────────────────────┤
│ TLV Chain (optional):                            │
│   Type(1) + Length(2) + Value(N)                │
│   AUTHORITY (0x02): SNI hostname                │
│   ALPN (0x01): 协议标识                          │
└─────────────────────────────────────────────────┘
```

**IPv4/IPv6 混合处理**：当 src 和 dst 地址族不同时，将 IPv4 地址映射为 IPv4-mapped IPv6（`::ffff:x.x.x.x`），统一使用 AF_INET6 格式。

**发送方式**：`write_all(pp2_header) + flush()` 确保 PP2 header 完整到达上游，然后才开始应用数据转发。

### 2.8 双向数据转发 (`duplex`)

使用 `tokio::select!` 实现全双工转发：

```
Client ──TLS──► [downstream buf 8KB] ──TCP──► Upstream
Client ◄──TLS── [upstream buf 8KB] ◄──TCP── Upstream
```

**特性：**

- 缓冲区大小：8192 bytes
- 即时 flush：每次 write_all 后立即 flush，降低延迟
- 字节计数：`ctx.bytes_sent` / `ctx.bytes_received` 精确统计
- 优雅关闭：任一方 EOF（read 返回 0）则退出循环
- 错误分类：区分 `UpstreamWriteError` / `DownstreamReadError` 等状态

## 3. 数据结构

### 3.1 TlsContext

连接级上下文，生命周期等于一个连接：

| 字段 | 类型 | 说明 |
|------|------|------|
| `listener_port` | `u16` | 接入端口 |
| `client_addr` | `String` | 客户端 IP |
| `client_port` | `u16` | 客户端端口 |
| `sni_hostname` | `Option<String>` | TLS SNI |
| `upstream_addr` | `Option<String>` | 上游地址 |
| `start_time` | `Instant` | 连接开始时间 |
| `bytes_sent` | `u64` | Client → Upstream 字节数 |
| `bytes_received` | `u64` | Upstream → Client 字节数 |
| `status` | `TlsStatus` | 连接结果状态 |
| `connection_established` | `bool` | 上游连接是否建立 |
| `proxy_protocol_sent` | `bool` | PP2 header 是否已发送 |
| `upstream_protocol` | `String` | 上游协议（当前固定 "TCP"） |
| `route_name` | `Option<String>` | 匹配的路由名称 `ns/name` |
| `gateway_key` | `Option<String>` | 所属 Gateway `ns/name` |

### 3.2 TlsStatus

连接结果枚举：

| 状态 | 触发条件 |
|------|---------|
| `Success` | 正常完成（初始值，duplex 正常结束） |
| `NoSniProvided` | TLS ClientHello 中无 SNI |
| `NoMatchingRoute` | SNI 无匹配的 TLSRoute |
| `UpstreamConnectionFailed` | 后端连接失败（含无可用 endpoint） |
| `UpstreamReadError` | 读取上游数据失败 |
| `UpstreamWriteError` | 写入上游数据失败（含 PP2 发送失败） |
| `DownstreamReadError` | 读取客户端数据失败 |
| `DownstreamWriteError` | 写入客户端数据失败 |
| `TlsHandshakeError` | TLS 握手失败（预留） |
| `DeniedByPlugin` | StreamPlugin 拒绝 |

### 3.3 EdgionTls

每个 TLS listener 一个实例，由 `listener_builder` 构造：

| 字段 | 类型 | 来源 |
|------|------|------|
| `gateway_name` | `String` | Gateway metadata |
| `gateway_namespace` | `Option<String>` | Gateway metadata |
| `listener_port` | `u16` | Listener 端口 |
| `gateway_tls_routes` | `Arc<GatewayTlsRoutes>` | `TlsRouteManager` 按 Gateway 分配 |
| `access_logger` | `Arc<AccessLogger>` | per-listener 日志 |
| `edgion_gateway_config` | `Arc<EdgionGatewayConfig>` | 全局 Gateway 配置 |
| `connector` | `TransportConnector` | Pingora TCP 连接器 |

## 4. 日志系统

### 4.1 双事件模型

每个连接产生两条日志：

| 事件 | 时机 | 包含信息 |
|------|------|---------|
| `connect` | 上游连接建立后 | 连接元数据（无 duration/bytes） |
| `disconnect` | 连接结束时 | 完整统计（duration/bytes/status） |

### 4.2 TlsLogEntry 字段

```json
{
  "ts": 1741296000000,
  "event": "connect",
  "protocol": "TLS-TCP",
  "listener_port": 31280,
  "client_addr": "127.0.0.1",
  "client_port": 52341,
  "sni_hostname": "test-443.sandbox.example.com",
  "upstream_addr": "10.244.1.10:30010",
  "status": "Success",
  "connection_established": true,
  "proxy_protocol": "v2",
  "route_name": "edgion-test/tls-route-basic",
  "gateway_name": "edgion-test/tls-route-basic-gw"
}
```

disconnect 事件额外包含：
```json
{
  "duration_ms": 1523,
  "bytes_sent": 8192,
  "bytes_received": 4096
}
```

### 4.3 日志双写

`log_disconnect` 同时写入两个目标：

1. **TLS 专用 logger**：通过 `log_tls()` 写入全局 `TLS_LOGGER`（`OnceLock<Arc<AccessLogger>>`），可独立配置输出路径。
2. **per-listener access_logger**：通过 `access_logger.send()` 写入 listener 级日志，与 HTTP/TCP/UDP 日志统一管理。

TLS logger 在 CLI 启动时通过 `init_tls_logger(&config.tls_log)` 初始化，配置来自 `EdgionGatewayConfig.tls_log`。

## 5. 配置面（控制面 → 数据面）

### 5.1 K8s 资源到数据面的映射

```
Gateway (spec.listeners[protocol=TLS])
   │
   ├─► listener_builder.rs → EdgionTls 实例
   │     └─ 绑定到 gateway_tls_routes
   │
   ├─► EdgionTls CRD → 证书绑定到 listener
   │
   └─► TLSRoute → conf_handler_impl.rs → GatewayTlsRoutes
         ├─ annotations → rule.proxy_protocol_version
         ├─ annotations → rule.upstream_tls
         ├─ annotations → rule.stream_plugin_store_key
         └─ backendRefs → rule.backend_finder (WeightedRR)
```

### 5.2 Annotation 参考

| Annotation | 位置 | 值 | 数据面影响 |
|-----------|------|-----|-----------|
| `edgion.io/proxy-protocol` | TLSRoute | `"v2"` | 向上游发送 PP2 header + AUTHORITY TLV |
| `edgion.io/upstream-tls` | TLSRoute | `"true"` | （预留）上游使用 TLS 连接 |
| `edgion.io/edgion-stream-plugins` | TLSRoute | `"ns/name"` | 关联 EdgionStreamPlugins 资源 |
| `edgion.io/backend-protocol` | Gateway | `"tcp"` | 标识后端协议 |

### 5.3 运行时字段

`TLSRouteRule` 上的三个 `#[serde(skip)]` runtime 字段由 `conf_handler_impl.rs` 在路由加载时填充：

```rust
pub struct TLSRouteRule {
    // ... K8s API 字段 ...
    
    #[serde(skip)]
    pub proxy_protocol_version: Option<u8>,      // from annotation
    #[serde(skip)]
    pub upstream_tls: bool,                       // from annotation
    #[serde(skip)]
    pub stream_plugin_store_key: Option<String>,  // from annotation
}
```

## 6. 源码清单

| 文件 | 职责 |
|------|------|
| `src/core/gateway/routes/tls/edgion_tls.rs` | 数据面主逻辑（ServerApp 实现） |
| `src/core/gateway/routes/tls/gateway_tls_routes.rs` | SNI 路由表（ArcSwap lock-free） |
| `src/core/gateway/routes/tls/routes_mgr.rs` | TlsRouteManager（按 Gateway 管理路由） |
| `src/core/gateway/routes/tls/conf_handler_impl.rs` | 配置处理（annotation 解析 + BackendSelector 初始化） |
| `src/core/common/utils/proxy_protocol.rs` | PP2 编码器（Builder 模式） |
| `src/core/gateway/observe/logs/tls_log.rs` | TLS 日志模块（全局单例 + 结构化 JSON） |
| `src/core/gateway/runtime/server/listener_builder.rs` | EdgionTls 实例构造（listener → ServerApp 绑定） |
| `src/types/resources/tls_route.rs` | TLSRoute 资源定义（含 runtime 字段） |
| `src/types/constants/annotations.rs` | Annotation 常量定义 |
