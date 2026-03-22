# TLS Passthrough 实现计划

## 设计概要

Passthrough listener 以**纯 TCP listener** 启动（不挂载 TLS 回调），在应用层手动解析 ClientHello 提取 SNI，然后复用现有 `TlsRouteTable` 做路由匹配，将完整的加密字节流透传到后端。

## 实现步骤

### Step 1: SNI 解析器

**新建文件:** `src/core/gateway/routes/tls/sni_parser.rs`

实现 `pub fn extract_sni(buf: &[u8]) -> Option<String>`，解析 TLS ClientHello 提取 SNI：

```
解析流程:
1. 验证 buf[0] == 0x16 (Handshake) 且 buf[5] == 0x01 (ClientHello)
2. 跳过 TLS Record Header (5 bytes)
3. 跳过 Handshake Header (4 bytes)
4. 跳过 ClientHello 固定部分: version(2) + random(32)
5. 跳过 session_id: len = buf[pos], skip 1 + len
6. 跳过 cipher_suites: len = u16(buf[pos..pos+2]), skip 2 + len
7. 跳过 compression_methods: len = buf[pos], skip 1 + len
8. 读取 extensions_length: u16(buf[pos..pos+2]), skip 2
9. 遍历 extensions:
   - type = u16(buf[pos..pos+2])
   - ext_len = u16(buf[pos+2..pos+4])
   - if type == 0x0000 (server_name):
     - skip list_length(2) + name_type(1)
     - name_len = u16
     - return hostname as UTF-8 string
   - else: skip 4 + ext_len
```

约 60-80 行代码，纯 `&[u8]` slice 操作，不需要额外依赖。

**包含单元测试:** 用真实 ClientHello 字节（可以用 openssl s_client 抓取）验证解析正确性，覆盖：
- 正常 TLS 1.2 ClientHello
- TLS 1.3 ClientHello
- 无 SNI 的 ClientHello
- 截断/畸形数据返回 None

---

### Step 2: Passthrough Proxy

**新建文件:** `src/core/gateway/routes/tls/passthrough_proxy.rs`

实现 `EdgionTlsPassthroughProxy`，结构参考 `EdgionTcpProxy`（更简洁）和 `EdgionTlsTcpProxy`（SNI 路由）：

```rust
pub struct EdgionTlsPassthroughProxy {
    pub listener_port: u16,
    pub tls_route_manager: Arc<TlsRouteManager>,  // 复用现有
    pub access_logger: Arc<AccessLogger>,
    pub edgion_gateway_config: Arc<EdgionGatewayConfig>,
    pub connector: TransportConnector,
}
```

**`process_new()` 流程:**

```
1. 从 stream peek 前 1024 字节 (try_peek 或 read + rewind)
2. 调用 sni_parser::extract_sni(buf)
3. 若无 SNI → 记录日志, 关闭连接
4. 用 tls_route_manager.load_route_table().match_route(sni) 匹配路由
5. 取第一个 rule, 选择 backend (BackendSelector)
6. TCP 连接 upstream
7. 双向透传 (tokio::io::copy_bidirectional 替代手写 duplex)
```

**关键点:**
- 不做 TLS 握手，不需要证书
- Stream 已经 rewind，透传包含完整 ClientHello
- 用 `tokio::io::copy_bidirectional` 替代手写 8KB buffer duplex（更高效，代码更少）
- Proxy Protocol v2 支持可选（复用 `send_proxy_protocol_v2` 逻辑）
- 日志记录连接级别信息（SNI、client IP、bytes、duration），无 HTTP 层信息

**Context 结构:** 复用或简化 `TlsContext`，去掉 mTLS 相关字段（Passthrough 无法获取客户端证书）。

---

### Step 3: Listener 分发

**修改文件:** `src/core/gateway/runtime/server/listener_builder.rs`

**3a. 新增 `add_tls_passthrough_listener()` 函数:**

```rust
pub fn add_tls_passthrough_listener(server: &mut Server, context: &ListenerContext) -> Result<()> {
    let port = context.listener.port;
    let tls_route_manager = get_global_tls_route_managers()
        .get_or_create_port_manager(port);

    let proxy = EdgionTlsPassthroughProxy {
        listener_port: port,
        tls_route_manager,
        access_logger: context.access_logger.clone(),
        edgion_gateway_config: context.edgion_gateway_config.clone(),
        connector: TransportConnector::new(None),
    };

    // 关键：用 Listeners::tcp() 而不是 add_tls_with_settings()
    let listeners = Listeners::tcp(&format!("0.0.0.0:{}", port));
    let mut service = Service::with_listeners(
        format!("TLS-Passthrough-{}", context.listener.name),
        listeners,
        proxy,
    );
    apply_connection_filter(&mut service, context);
    server.add_service(service);
    Ok(())
}
```

**3b. 修改 `add_listener()` 分发逻辑:**

```rust
"TLS" => {
    let mode = context.listener.tls.as_ref()
        .and_then(|t| t.mode.as_deref())
        .unwrap_or("Terminate");
    match mode {
        "Passthrough" => add_tls_passthrough_listener(server, &context),
        _ => add_tls_terminate_to_tcp_listener(server, &context),
    }
}
```

---

### Step 4: 模块导出

**修改文件:** `src/core/gateway/routes/tls/mod.rs`

- 添加 `mod sni_parser;` 和 `mod passthrough_proxy;`
- 导出 `EdgionTlsPassthroughProxy`

---

### Step 5: Controller 侧适配（可选优化）

**文件:** `src/core/controller/sync_runtime/resource_processor/handlers/gateway.rs`

- Passthrough 模式下 `certificateRefs` 不是必须的
- 可以在 listener 校验时：Passthrough 模式跳过 Secret 解析，避免因无证书报 ResolvedRefs=False
- 若 Passthrough 模式配置了 certificateRefs，可以发 warning（配了也不用）

---

## 文件变更清单

| 文件 | 操作 | 说明 |
|------|------|------|
| `src/core/gateway/routes/tls/sni_parser.rs` | 新建 | ClientHello SNI 解析器 + 测试 |
| `src/core/gateway/routes/tls/passthrough_proxy.rs` | 新建 | Passthrough proxy 实现 |
| `src/core/gateway/routes/tls/mod.rs` | 修改 | 添加模块导出 |
| `src/core/gateway/runtime/server/listener_builder.rs` | 修改 | 添加 passthrough listener + 分发逻辑 |
| `src/core/controller/.../handlers/gateway.rs` | 修改 | Passthrough 模式跳过证书解析（可选） |

## 验证

- `cargo build` 编译通过
- `cargo clippy` 无新增警告
- `cargo test` 通过（含 sni_parser 单元测试）
- 现有 TLS Terminate 功能不受影响
