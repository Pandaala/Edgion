# TLS Passthrough 模式实现

优先级: P0

## 现状

- `GatewayTLSConfig.mode` 接受字符串值，可以设置 "Passthrough"
- Gateway listener 只实现了 Terminate 模式
- TLSRoute 在 `proxy.rs` 中实现为 `EdgionTlsTcpProxy`，总是先终止 TLS 再转发明文 TCP
- grep "Passthrough" 在 gateway 代码库中无命中
- TLSRoute 仍注册为 `v1alpha2`（应更新到 `v1`，GA v1.5）

## 规范要求

- **Passthrough**: Gateway 不终止 TLS，将原始加密流量直接转发到后端。后端负责 TLS 终止
- **Terminate**: Gateway 终止 TLS，将解密后的流量转发到后端
- TLSRoute 在 Passthrough 模式下基于 SNI 做路由，但不解密流量

## 需要实现

### 1. Listener 层面

- 在 Gateway listener 初始化时根据 `tls.mode` 决定是否启用 TLS 终止
- Passthrough 模式下，只提取 ClientHello 中的 SNI（不需要证书），不执行 TLS 握手
- 需要实现 SNI 提取（读取 ClientHello 前几个字节解析 SNI）

### 2. TLSRoute Passthrough 转发

- SNI 匹配后，直接将原始 TCP 连接透传到后端
- 使用双向字节拷贝（`tokio::io::copy_bidirectional`）
- 不需要 upstream TLS 配置（后端自己处理）

### 3. 与 Terminate 模式共存

- 同一 Gateway 可以有多个 listener，部分 Terminate 部分 Passthrough
- 需要在 listener 初始化时区分两种模式的处理路径

### 4. TLSRoute API 版本更新

- `src/types/resources/tls_route.rs:27` 的 `api_version` 从 `v1alpha2` 更新到 `v1`

## 涉及文件

- `src/types/resources/tls_route.rs` — API 版本更新
- `src/types/resources/gateway.rs` — listener TLS mode 处理
- `src/core/gateway/routes/tls/proxy.rs` — 添加 Passthrough 转发路径
- `src/core/gateway/tls/` — SNI-only 提取（无完整握手）
- `src/core/gateway/runtime/server.rs` — listener 初始化分支
