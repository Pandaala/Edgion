# TLS Gateway 集成测试详解

> TLSRoute 的三个集成测试套件：Basic、ProxyProtocol、StreamPlugins 的完整配置、测试逻辑、运行方式和排查指南。

## 1. 测试架构

```
run_integration.sh
  └─► test_client -g -r TLSRoute -i <suite>
        ├─ Basic         → TlsRouteTestSuite
        ├─ ProxyProtocol → TlsProxyProtocolTestSuite
        └─ StreamPlugins → TlsStreamPluginsTestSuite
```

**三个维度：**

| 套件 | 验证内容 | 端口 |
|------|---------|------|
| Basic | TLS 终止 + SNI 路由 + TCP 转发 | 31280 |
| ProxyProtocol | PP2 header 编码 + AUTHORITY TLV + 后端解析 | 31281 (+ 31280 反向) |
| StreamPlugins | 路由级 StreamPlugin 允许/拒绝 + 短名解析 | 31282, 31283 |

## 2. 测试环境

### 2.1 服务组件

```
┌──────────────────────────────────────┐
│           Integration Test           │
│                                      │
│ edgion-gateway  ◄─── K8s config      │
│   ├─ TLS listener :31280 (Basic)     │
│   ├─ TLS listener :31281 (PP2)       │
│   ├─ TLS listener :31282 (SP allow)  │
│   └─ TLS listener :31283 (SP deny)   │
│                                      │
│ test_server                          │
│   ├─ TCP echo :30010                 │
│   └─ TCP-PP2  :30012 (PP2 解析)      │
│                                      │
│ test_client                          │
│   └─ TLS 客户端 (tokio-rustls)       │
└──────────────────────────────────────┘
```

### 2.2 TLS 客户端

所有测试共用 `make_tls_connector()` 创建 TLS 连接器：

- 基于 `tokio-rustls`（项目已有的依赖）
- 使用 `NoCertVerifier` 跳过证书验证（测试环境使用自签名证书）
- `NoCertVerifier` 在 `basic/basic.rs` 中定义，`pub(crate)` 可见性，供所有 TLS 测试套件共享

### 2.3 端口分配

来自 `examples/test/conf/ports.json`：

```json
"TLSRoute/Basic":         { "tls": 31280 },
"TLSRoute/ProxyProtocol": { "tls": 31280, "tls_pp2": 31281 },
"TLSRoute/StreamPlugins": { "tls": 31282, "tls_filtered": 31283 }
```

### 2.4 后端服务

| 端口 | 类型 | 说明 |
|------|------|------|
| 30010 | TCP Echo | 原样回传接收到的数据 |
| 30012 | TCP PP2-aware | 使用 `proxy-header` crate 解析 PP2 header，返回结构化 JSON |

`test_server` 启动参数（在 `start_all_with_conf.sh` 中）：

```bash
test_server ... --tcp-pp2-port 30012
```

## 3. Suite 1: Basic

### 3.1 K8s 配置

目录：`examples/test/conf/TLSRoute/Basic/`

| 文件 | 资源 | 说明 |
|------|------|------|
| `01_Gateway.yaml` | Gateway `tls-route-basic-gw` | 单 listener `tls-basic`，port 31280，hostname `*.sandbox.example.com`，TLS Terminate |
| `02_EdgionTls.yaml` | EdgionTls `tls-route-basic-cert` | 绑定通配符证书 `edge-tls` 到 listener `tls-basic`，hosts `*.sandbox.example.com` |
| `03_TLSRoute.yaml` | TLSRoute `tls-route-basic` | 匹配 `*.sandbox.example.com` → backend `test-tcp:30010` |
| `Service_test-tcp.yaml` | Service `test-tcp` | ClusterIP None，port 30010 |
| `EndpointSlice_test-tcp.yaml` | EndpointSlice | 指向 `127.0.0.1:30010`（本地 test_server） |

**资源依赖链：**

```
Gateway (tls-route-basic-gw)
  └─ Listener (tls-basic, port 31280, TLS Terminate)
       ├─ EdgionTls (tls-route-basic-cert) → Secret (edge-tls)
       └─ TLSRoute (tls-route-basic)
            └─ BackendRef → Service (test-tcp) → EndpointSlice → 127.0.0.1:30010
```

### 3.2 测试用例

#### `tls_route_connection`

验证 TLS 终止 + SNI 路由 + TCP 转发的完整链路。

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | TCP 连接 `127.0.0.1:31280` | 连接成功 |
| 2 | TLS 握手，SNI = `test-443.sandbox.example.com` | 握手成功（证书匹配 `*.sandbox.example.com`） |
| 3 | 发送 `"Hello TLSRoute"` | 写入成功 |
| 4 | 读取响应 | 收到完全相同的 `"Hello TLSRoute"`（echo） |

**数据流：**
```
test_client ──TLS(SNI=test-443.sandbox.example.com)──► :31280 Gateway
  Gateway: TLS terminate → match TLSRoute → select test-tcp:30010
  Gateway ──TCP──► :30010 test_server (echo)
  Gateway ◄──TCP── :30010 response
test_client ◄──TLS── :31280 response
```

#### `tls_route_sni_mismatch`

验证不匹配的 SNI 被拒绝。

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | TCP 连接 `127.0.0.1:31280` | 连接成功 |
| 2 | TLS 握手，SNI = `nomatch.other.com` | 握手可能成功（SNI 不影响证书选择）或失败 |
| 3 | 发送数据 | - |
| 4 | 读取响应 | EOF / 连接关闭 / 读取错误（均视为 PASS） |

**通过条件**：以下任一情况视为通过：
- TLS 握手失败
- 读到 0 字节（EOF）
- 读取返回错误
- 读取超时

## 4. Suite 2: ProxyProtocol

### 4.1 K8s 配置

目录：`examples/test/conf/TLSRoute/ProxyProtocol/`

| 文件 | 资源 | 说明 |
|------|------|------|
| `01_Gateway.yaml` | Gateway `tls-route-pp2-gw` | 单 listener `tls-pp2`，port 31281，hostname `*.pp2.example.com` |
| `02_EdgionTls.yaml` | EdgionTls `tls-pp2-gw-pp2-cert` | 绑定证书到 `tls-pp2` listener |
| `03_TLSRoute_pp2.yaml` | TLSRoute `tls-pp2-route` | 带 `edgion.io/proxy-protocol: "v2"` annotation → `test-tcp-pp2:30012` |
| `Service_test-tcp-pp2.yaml` | Service `test-tcp-pp2` | PP2 后端，port 30012 |
| `EndpointSlice_test-tcp-pp2.yaml` | EndpointSlice | 指向 `127.0.0.1:30012` |

**配置要点**：
- PP2 路由使用独立的 hostname 域（`*.pp2.example.com`），与 Basic 的 `*.sandbox.example.com` 隔离
- PP2 路由指向专用的 PP2-aware 后端 `test-tcp-pp2:30012`（不是普通 echo 30010）
- 反向测试（test_no_pp2_without_annotation）复用 Basic suite 的 31280 端口和配置

### 4.2 PP2-aware 后端

`test_server` 的 `start_tcp_pp2_server` 实现了完整的 PP2 解析：

1. 接收 TCP 连接
2. 读取首包数据（可能包含 PP2 header + app data）
3. 使用 `proxy_header::ProxyHeader::parse()` 解析 PP2 header
4. 成功时返回 JSON 响应：

```json
{
  "pp2": true,
  "src_addr": "127.0.0.1:52341",
  "dst_addr": "127.0.0.1:30012",
  "authority": "test-443.pp2.example.com",
  "peer_addr": "127.0.0.1:54321",
  "pp2_header_len": 54
}
```

5. 如果首包数据不足（`BufferTooShort`），会再读一次重试
6. 非 PP2 连接返回 `{"pp2": false, "error": "..."}`

### 4.3 测试用例

#### `tls_route_pp2_header_parsed`

验证 PP2 header 被正确编码、发送到后端并解析。

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | TLS 连接 `:31281`，SNI = `test-443.pp2.example.com` | 握手成功 |
| 2 | 发送 `"PP2-CHECK"` | 触发 Gateway 转发 PP2 + 数据到后端 |
| 3 | 读取后端响应 | 收到 JSON |
| 4 | 验证 `pp2 == true` | PP2 header 被识别 |
| 5 | 验证 `authority == "test-443.pp2.example.com"` | AUTHORITY TLV 携带正确 SNI |
| 6 | 验证 `src_addr` 非空且不是 "local" | 源地址被正确编码 |

**数据流：**
```
test_client ──TLS(SNI=test-443.pp2.example.com)──► :31281 Gateway
  Gateway: TLS terminate → match TLSRoute (has PP2 annotation)
  Gateway ──TCP──► [PP2 header + "PP2-CHECK"] ──► :30012 test_server (PP2-aware)
  test_server: parse PP2 → extract src/dst/authority → return JSON
test_client ◄──TLS── JSON response
```

#### `tls_route_no_pp2_without_annotation`

验证无 PP2 annotation 的 TLSRoute 不会发送 PP2 header（反向对比测试）。

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | TLS 连接 `:31280`（Basic 端口），SNI = `test-443.sandbox.example.com` | 握手成功 |
| 2 | 发送 `"NO-PP2-TEST"` | |
| 3 | 读取响应 | 收到原始 echo 数据 |
| 4 | 验证响应 == `"NO-PP2-TEST"` | 无 PP2 header 前缀 |

**通过条件**：
- 响应完全等于发送数据（无 PP2 前缀）
- 或响应不以 PP2 签名（12 字节 `\r\n\r\n\0\r\nQUIT\n`）开头

**注意**：此测试依赖 Basic suite 的配置也被加载。它连接的是 Basic 的 Gateway（31280），后端是普通 echo server（30010）。

## 5. Suite 3: StreamPlugins

### 5.1 K8s 配置

目录：`examples/test/conf/TLSRoute/StreamPlugins/`

| 文件 | 资源 | 说明 |
|------|------|------|
| `01_EdgionStreamPlugins_deny.yaml` | EdgionStreamPlugins `tls-deny-all` | IpRestriction, defaultAction: deny |
| `02_EdgionStreamPlugins_allow_localhost.yaml` | EdgionStreamPlugins `tls-allow-localhost` | IpRestriction, defaultAction: deny, allowList: 127.0.0.0/8 |
| `03_Gateway.yaml` | Gateway `tls-stream-plugins-gw` | 两个 listener：`tls-allow-localhost` (:31282)，`tls-deny-all` (:31283) |
| `03b_EdgionTls_allow.yaml` | EdgionTls `tls-sp-allow-cert` | 绑定证书到 `tls-allow-localhost` listener |
| `03c_EdgionTls_deny.yaml` | EdgionTls `tls-sp-deny-cert` | 绑定证书到 `tls-deny-all` listener |
| `04_TLSRoute_allow_localhost.yaml` | TLSRoute `tls-sp-allow-localhost` | annotation: `edgion.io/edgion-stream-plugins: "edgion-test/tls-allow-localhost"` |
| `05_TLSRoute_deny.yaml` | TLSRoute `tls-sp-deny-all` | annotation: `edgion.io/edgion-stream-plugins: "tls-deny-all"`（短名格式） |
| `Service_test-tcp.yaml` | Service `test-tcp` | 后端 |
| `EndpointSlice_test-tcp.yaml` | EndpointSlice | 指向 127.0.0.1:30010 |

**关键设计**：每个 TLS listener 必须有对应的 `EdgionTls` 资源来绑定证书，否则 TLS 握手后连接会被 reset。

### 5.2 EdgionStreamPlugins 配置

**deny-all**（无 allowList）：
```yaml
spec:
  plugins:
    - type: IpRestriction
      config:
        defaultAction: deny
```

**allow-localhost**（允许 127.0.0.0/8）：
```yaml
spec:
  plugins:
    - type: IpRestriction
      config:
        defaultAction: deny
        allowList:
          - "127.0.0.0/8"
```

### 5.3 测试用例

#### `tls_stream_plugin_allow_localhost`

验证 allow-localhost 插件允许来自 127.0.0.1 的连接。

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | TLS 连接 `:31282`，SNI = `test.sp-allow.example.com` | 握手成功 |
| 2 | 发送 `"Hello TLS StreamPlugin"` | 写入成功 |
| 3 | 读取响应 | 收到 echo 数据（StreamPlugin 允许） |

**Plugin 执行链**：
```
TLS handshake → extract SNI → match TLSRoute (has stream_plugin_store_key)
  → lookup EdgionStreamPlugins "edgion-test/tls-allow-localhost"
  → IpRestriction: client 127.0.0.1 in allowList 127.0.0.0/8 → Allow
  → connect upstream → duplex
```

#### `tls_stream_plugin_deny_all`

验证 deny-all 插件拒绝连接。

| 步骤 | 操作 | 预期 |
|------|------|------|
| 1 | TLS 连接 `:31283`，SNI = `test.sp-deny.example.com` | 握手成功 |
| 2 | 发送 `"Hello"` | |
| 3 | 读取响应 | EOF / 连接错误（Plugin 拒绝） |

**通过条件**：
- 收到 0 字节（EOF）
- 读取返回错误
- TLS 握手失败
- 连接被拒绝/超时

#### `tls_stream_plugin_short_name`

验证短名 annotation 格式（不含 namespace/）能正确解析。

**测试目标**：`05_TLSRoute_deny.yaml` 中的 annotation：
```yaml
edgion.io/edgion-stream-plugins: "tls-deny-all"
```
应自动补上 namespace 前缀变为 `edgion-test/tls-deny-all`，匹配到正确的 EdgionStreamPlugins。

行为与 `deny_all` 测试相同：连接应被拒绝。

## 6. 运行方式

### 6.1 全量运行

```bash
./examples/test/scripts/integration/run_integration.sh
```

自动运行所有测试套件，包括 `TLSRoute_Basic`、`TLSRoute_ProxyProtocol`、`TLSRoute_StreamPlugins`。

### 6.2 单独运行 TLSRoute 测试

```bash
./examples/test/scripts/integration/run_integration.sh -t TLSRoute
```

会运行 TLSRoute 下所有三个子套件。

### 6.3 运行单个子套件

```bash
# 直接使用 test_client
./target/debug/examples/test_client -g -r TLSRoute -i Basic
./target/debug/examples/test_client -g -r TLSRoute -i ProxyProtocol
./target/debug/examples/test_client -g -r TLSRoute -i StreamPlugins
```

CLI 别名支持：
- `tls` / `tls-route` / `tlsroute` → `TLSRoute/Basic`
- `tls-pp2` / `tlspp2` / `tls-proxy-protocol` → `TLSRoute/ProxyProtocol`
- `tls-stream-plugins` / `tlsstreamplugins` → `TLSRoute/StreamPlugins`

## 7. 配置文件目录结构

```
examples/test/conf/TLSRoute/
├── Basic/
│   ├── 01_Gateway.yaml
│   ├── 02_EdgionTls.yaml
│   ├── 03_TLSRoute.yaml
│   ├── Service_test-tcp.yaml
│   └── EndpointSlice_test-tcp.yaml
├── ProxyProtocol/
│   ├── 01_Gateway.yaml
│   ├── 02_EdgionTls.yaml
│   ├── 03_TLSRoute_pp2.yaml
│   ├── Service_test-tcp-pp2.yaml
│   └── EndpointSlice_test-tcp-pp2.yaml
└── StreamPlugins/
    ├── 01_EdgionStreamPlugins_deny.yaml
    ├── 02_EdgionStreamPlugins_allow_localhost.yaml
    ├── 03_Gateway.yaml
    ├── 03b_EdgionTls_allow.yaml
    ├── 03c_EdgionTls_deny.yaml
    ├── 04_TLSRoute_allow_localhost.yaml
    ├── 05_TLSRoute_deny.yaml
    ├── Service_test-tcp.yaml
    └── EndpointSlice_test-tcp.yaml
```

K8s 测试配置在 `examples/k8stest/conf/TLSRoute/` 下，结构相同但不含 `EndpointSlice`（K8s 自动管理）。

## 8. 测试代码结构

```
examples/code/client/suites/tls_route/
├── mod.rs                              # 模块入口，导出三个 TestSuite
├── basic/
│   ├── mod.rs
│   └── basic.rs                        # TlsRouteTestSuite + make_tls_connector + NoCertVerifier
├── proxy_protocol/
│   ├── mod.rs
│   └── proxy_protocol.rs              # TlsProxyProtocolTestSuite
└── stream_plugins/
    ├── mod.rs
    └── stream_plugins.rs              # TlsStreamPluginsTestSuite
```

## 9. 排查指南

### 9.1 TLS 握手失败（Connection reset by peer）

**症状**：`TLS handshake failed` 或 `Read failed: Connection reset by peer`

**常见原因**：

1. **缺少 EdgionTls 资源**：每个 TLS listener 都需要对应的 `EdgionTls` CRD 将证书绑定到该 listener。Gateway 的 `certificateRefs` 声明证书引用，但 `EdgionTls` 是实际执行绑定的资源。

2. **hostname 不匹配**：TLSRoute 的 `hostnames` 必须在 Gateway listener 的 `hostname` 范围内。例如 Gateway 监听 `*.sandbox.example.com`，TLSRoute 只能匹配该域下的子域。

3. **证书名称不匹配**：`EdgionTls` 的 `hosts` 必须覆盖 TLSRoute 的 `hostnames`。

**排查步骤**：
```bash
# 确认 Gateway listener 存在
grep -r "protocol: TLS" examples/test/conf/TLSRoute/<suite>/

# 确认 EdgionTls 绑定了对应 listener
grep -r "sectionName:" examples/test/conf/TLSRoute/<suite>/*EdgionTls*

# 确认 hostname 范围匹配
grep -r "hostname" examples/test/conf/TLSRoute/<suite>/
```

### 9.2 PP2 测试失败

**症状**：`PP2 not detected by backend` 或 `AUTHORITY TLV mismatch`

**排查**：
1. 确认 `test_server` 带 `--tcp-pp2-port 30012` 参数启动
2. 确认 TLSRoute 有 `edgion.io/proxy-protocol: "v2"` annotation
3. 确认 TLSRoute 的 backend 指向 `test-tcp-pp2:30012`（不是普通 echo 30010）
4. 确认 PP2 路由的 hostname 域在 Gateway listener 范围内

### 9.3 StreamPlugin 测试 allow 失败但 deny 通过

**症状**：`tls_stream_plugin_allow_localhost` 失败，`deny_all` 和 `short_name` 通过

**原因**：deny/short_name 测试将连接错误也视为 PASS（因为预期就是拒绝），所以即使配置有问题也会"通过"。而 allow 测试必须成功收到 echo 数据才算 PASS。

**常见根因**：
- 缺少 EdgionTls 证书绑定（见 9.1）
- StreamPlugin 资源未加载（store_key 找不到对应资源，默认允许连接）
- 后端 echo server 未启动

### 9.4 测试套件未执行

**症状**：`run_integration.sh` 不包含 TLSRoute 测试

**排查**：
1. 确认 `run_integration.sh` 中有 `TLSRoute)` case
2. 确认 `test_client.rs` 中有 TLSRoute 的 CLI 映射
3. 确认 `suites/mod.rs` 导出了 `TlsRouteTestSuite`、`TlsProxyProtocolTestSuite`、`TlsStreamPluginsTestSuite`

## 10. 测试覆盖矩阵

| 功能点 | Basic | PP2 | StreamPlugins |
|--------|-------|-----|---------------|
| TLS 终止 | ✓ | ✓ | ✓ |
| SNI 路由匹配 | ✓ | ✓ | ✓ |
| SNI 不匹配拒绝 | ✓ | | |
| TCP 转发 echo | ✓ | | ✓ |
| PP2 header 编码 | | ✓ | |
| AUTHORITY TLV (SNI) | | ✓ | |
| PP2 源地址 | | ✓ | |
| 无 PP2 annotation 反向 | | ✓ | |
| IpRestriction 允许 | | | ✓ |
| IpRestriction 拒绝 | | | ✓ |
| 短名 annotation 解析 | | | ✓ |

**单元测试覆盖**（不在集成测试范围，但有对应 `#[cfg(test)]`）：

| 模块 | 单元测试 |
|------|---------|
| `proxy_protocol.rs` | IPv4/IPv6/混合地址族/AUTHORITY TLV/多 TLV/已知字节验证（6 个） |
| `conf_handler_impl.rs` | full_set/partial_update/PP2 annotation/upstream_tls/stream_plugin 全路径和短名/组合 annotation/无 annotation（9 个） |
