---
name: gateway-downstream-tls
description: 下游 TLS（客户端→网关）：Pingora Listener TLS 回调、证书选择、客户端证书验证。
---

# 下游 TLS（客户端→网关）

## 概述

下游 TLS 处理客户端到 Gateway 的 TLS 握手。核心实现在 `TlsCallback` 结构中，它实现了 Pingora 的 `TlsAccept` trait，在 TLS 握手的关键阶段注入自定义逻辑。

## Pingora TLS 回调

`TlsCallback` 实现 `TlsAccept` trait 的两个回调：

### certificate_callback

在 TLS 握手期间被调用（ClientHello 处理后），负责：

1. **SNI 解析**：
   - 从 `ssl.servername(NameType::HOST_NAME)` 获取 SNI。
   - 若无 SNI，检查 `EdgionGatewayConfig` 的 `security_protect.fallback_sni` 配置。
   - 两者均无则记录 `"no SNI"` 错误并中止。

2. **证书匹配与应用**（`match_and_apply_cert`）：
   - Layer 1：`TlsCertMatcher::match_sni_with_port(port, sni)` 查找 EdgionTls。
   - Layer 2：`GatewayTlsMatcher::match_sni_with_port(port, sni)` 查找 Gateway TLS。
   - 匹配成功后应用证书和私钥到 SSL 连接。

3. **SslCtx 存储**：
   - 创建 `SslCtx`（per-connection TLS 上下文），包含 `TlsConnMeta` 元信息。
   - 通过 BoringSSL `SSL_set_ex_data` 将 `SslCtx` 附加到 SSL 对象上。
   - 生成 `tls_id`（时间戳 + 随机数的十六进制标识符）用于日志关联。

### handshake_complete_callback

TLS 握手完成后被调用，负责：

1. 通过 `SSL_get_ex_data` 取回 `SslCtx`。
2. 若为 mTLS 连接且配置了客户端证书暴露，提取客户端证书信息。
3. 记录握手完成时间。
4. 输出 SSL 日志。
5. 返回 `Arc<TlsConnMeta>` 作为连接 digest 的扩展数据，供后续 HTTP/TLS 代理访问。

## 证书选择逻辑

### EdgionTls 证书应用

`apply_edgion_tls_cert` 处理 EdgionTls 资源的证书：

1. 从 `EdgionTls` 提取 cert PEM 和 key PEM。
2. 解析为 `X509` 和 `PKey`，通过 Pingora TLS 扩展 API 设置到 SSL 连接。
3. 可选配置：
   - **mTLS**（`configure_mtls`）：设置 CA 证书存储、验证模式、验证深度。
   - **最低 TLS 版本**（`configure_min_tls_version`）：TLS 1.0 ~ 1.3。
   - **密码套件**（`configure_ciphers`）：通过 BoringSSL FFI 设置。

### Gateway TLS 证书应用

`apply_gateway_tls_cert` 处理 Gateway Listener 的 TLS 证书：

1. 从 `GatewayTlsEntry` 的内联 `secrets` 获取 Secret（优先）。
2. 回退到从全局 SecretStore 按 `certificateRefs` 查找 Secret。
3. 从 Secret 的 `data["tls.crt"]` 和 `data["tls.key"]` 提取证书和私钥。
4. 解析并设置到 SSL 连接（不支持 mTLS 等高级配置）。

## mTLS 客户端验证

当 EdgionTls 配置了 `spec.client_auth` 时，`configure_mtls` 启用客户端证书验证：

### 验证模式

| 模式 | SslVerifyMode | 说明 |
|------|--------------|------|
| `Terminate` | 无验证 | 忽略客户端证书，仅终止 TLS |
| `Mutual` | `PEER + FAIL_IF_NO_PEER_CERT` | 必须提供有效客户端证书 |
| `OptionalMutual` | `PEER` | 有证书则验证，无证书也放行 |

### 配置流程

1. 从 EdgionTls 获取 CA 证书 PEM，构建 `X509Store`。
2. 设置验证深度（`verify_depth`，范围 1-9）。
3. 若配置了 `allowed_sans` 或 `allowed_cns`，设置自定义验证回调（`set_mtls_verify_callback`）进行白名单校验。
4. 否则使用标准 OpenSSL/BoringSSL 验证链。

### 客户端证书信息提取

握手完成后，`extract_client_cert_info` 从 SSL 连接中提取客户端证书信息：

- Subject DN（Distinguished Name）
- Common Name（CN）
- Subject Alternative Names（DNS、IP、Email、URI）
- SHA256 指纹

提取的 `ClientCertInfo` 存储在 `TlsConnMeta.client_cert_info` 中，可供插件和访问日志使用。

## SslCtx 与 ex_data

per-connection TLS 上下文通过 BoringSSL ex_data 机制在回调之间传递：

```
certificate_callback          handshake_complete_callback
       │                                │
       ├── SslCtx::new()                ├── take_ssl_ctx(ssl)
       ├── ... 填充数据 ...             ├── 提取客户端证书
       └── store_ssl_ctx(ssl, ctx)      └── 返回 Arc<TlsConnMeta>
```

- `SSL_get_ex_new_index`：注册自定义 ex_data 索引（全局一次）。
- `SSL_set_ex_data`：存储 `Box<SslCtx>` 指针。
- `SSL_get_ex_data`：取回并转移所有权。
- 注册的 `free_ssl_ctx` 回调在 SSL 对象销毁时释放未被取回的 SslCtx。

非 BoringSSL 构建中，ex_data 操作为空操作，`handshake_complete_callback` 返回基础 `TlsConnMeta`。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | TlsCallback、SslCtx、证书应用 |
| `src/core/gateway/tls/store/cert_matcher.rs` | TlsCertMatcher（EdgionTls 匹配） |
| `src/core/gateway/runtime/matching/tls.rs` | GatewayTlsMatcher（Gateway 回退匹配） |
| `src/core/gateway/tls/runtime/backend/cert_extractor.rs` | 客户端证书信息提取 |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | BoringSSL mTLS 验证回调 |
