---
name: gateway-downstream-tls
description: 下游 TLS（客户端→网关）：Pingora Listener TLS 回调、证书选择、客户端证书验证。
---

# 下游 TLS（客户端→网关）

## 概述

下游 TLS 处理客户端到 Gateway 的 TLS 握手。核心实现在 `TlsCallback` 结构中，它实现了 Pingora 的 `TlsAccept` trait，在 TLS 握手的关键阶段注入自定义逻辑。

## Pingora TLS 回调

`TlsCallback` 实现 `TlsAccept` trait 的两个回调，并额外通过 BoringSSL FFI 注册一个早期回调。三个回调按 BoringSSL 状态机顺序依次触发：

### select_certificate_callback（早期回调，Phase 1 引入）

注册位置：`new_tls_settings_with_callback` 中通过 `TlsSettings: DerefMut<SslAcceptorBuilder>` 直接调用 BoringSSL `SSL_CTX_set_select_certificate_cb`，无需 pingora 补丁。

触发时机：BoringSSL `handshake_server.cc:694`，**早于** `L710` 的版本范围 freeze，是 `min_tls_version` 唯一真实生效的时机。

职责：

1. 从 ClientHello 提取 SNI（`ch.servername(NameType::HOST_NAME)`）。
2. Layer 1 匹配：`TlsCertMatcher::match_sni(listener_key, sni)` 查 EdgionTls。
3. 命中且 `et.spec.min_tls_version` 有值 → `ssl.set_min_proto_version(...)` per-connection 设置最低版本。
   - 失败时 fail-closed：写 `err_log = "min ver: ..."` + `log_ssl` + 返回 `Err(SelectCertError::ERROR)`。
4. 通过 `store_ssl_ctx` 把匹配结果（`Arc<EdgionTls>` + 已 resolve 的 SNI）写入 SSL ex_data 作为快照。
5. 除版本设置失败外，始终返回 `Ok(())`，让握手继续到 cert_cb。

为什么不能在 cert_cb 里设置 `min_tls_version`：BoringSSL `handshake_server.cc:710` 明确——版本范围在 select_cb 之后立即冻结，cert_cb（`L759`）到达时已不可更改。

### certificate_callback（cert_cb，handshake_server.cc:759）

在 TLS 握手期间被调用（版本协商后、密码套件协商前），负责：

1. **取出快照**：`take_ssl_ctx` 消费式取出 select_cb 写入的 ex_data 快照（自动清空槽）。

2. **快速路径**：若快照中 `matched_edgion_tls.is_some()` → 直接调用 `apply_edgion_tls_cert`，**不再执行 `match_sni`**——单点匹配避免 select_cb / cert_cb 双 match 在 hot-reload 窗口内拿到不同代资源。

3. **SNI 解析**（无快照或快照无命中时）：
   - 优先使用快照中保存的 `sni`。
   - 回退到 `ssl.servername(NameType::HOST_NAME)`。
   - 最终回退到 `EdgionGatewayConfig` 的 `security_protect.fallback_sni`。
   - 三者均无则记录 `"no SNI"` 错误并中止。

4. **重试 Layer 1**（防御性，覆盖非 boringssl 后端 / select_cb store 失败）：`TlsCertMatcher::match_sni`。

5. **Layer 2**：`GatewayTlsMatcher::match_sni(key: &ListenerKey, sni)` 查找 Gateway TLS。

6. **均未命中**：写 `err_log = "cert not found"` + `log_ssl`。

7. **命中后收尾**：调用 `finalize_ssl_ctx` 盖 `tls_id`（时间戳 + 随机数的十六进制标识符），并通过 `store_ssl_ctx` 把更新后的 `SslCtx` 写回 ex_data，供 `handshake_complete_callback` 取用。

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
3. 可选配置（仅在 cert_cb 时机内有效的项）：
   - **mTLS**（`configure_mtls`）：设置 CA 证书存储、验证模式、验证深度。
   - **密码套件**（`configure_ciphers`）：通过 BoringSSL FFI 设置 TLS1.2 cipher list（cert_cb 仍有效，因为 BoringSSL 在 cert_cb 之后才选 cipher——`handshake_server.cc:810`）。

> **`min_tls_version` 的生效位置已移至 `select_certificate_callback`**（见上节）。cert_cb 到达时版本范围已被 BoringSSL 冻结，在此处调用 `set_min_proto_version` 静默无效，因此 `apply_edgion_tls_cert` 不再处理该字段。

### Gateway TLS 证书应用

`apply_gateway_tls_cert` 处理 Gateway Listener 的 TLS 证书：

1. 从 `GatewayTlsEntry` 的内联 `secrets` 获取 Secret（优先）。
2. 回退到从全局 SecretStore 按 `certificateRefs` 查找 Secret。
3. 从 Secret 的 `data["tls.crt"]` 和 `data["tls.key"]` 提取证书和私钥。
4. 解析并设置到 SSL 连接。
5. 若 `frontend_validation` 存在，调用 `configure_gateway_frontend_mtls()` 配置客户端证书验证。

## Gateway frontendValidation（标准 mTLS）

Gateway API 标准的前端客户端证书验证，通过 `spec.tls.frontend` 配置。与 EdgionTls `client_auth` 完全独立。

### 验证模式

| 模式 | 行为 | 实现 |
|------|------|------|
| `AllowValidOnly`（默认） | 必须提供匹配 CA 签发的有效证书 | `PEER \| FAIL_IF_NO_PEER_CERT` |
| `AllowInsecureFallback` | 有效/无效/缺失证书均放行 | 自定义 verify callback 始终返回 1（boringssl FFI） |

### 配置流程（`configure_gateway_frontend_mtls`）

1. 从 `resolved_frontend_ca_secrets` 提取 CA 证书（key: `ca.crt`），构建 `X509Store`。
2. 根据 `mode` 选择 verify mode：AllowValidOnly 用标准 SSL verify，AllowInsecureFallback 用 `boring_sys::SSL_set_verify` + `insecure_fallback_verify_cb`。
3. 设置 `verify_depth(10)`。

per-listener 配置由 Controller 合并到每个 listener 的 `GatewayTlsConfig` 中，Gateway 侧无需额外处理。

## EdgionTls mTLS 客户端验证

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
select_certificate_callback   certificate_callback          handshake_complete_callback
       │                              │                                │
       ├── SslCtx::new()              ├── take_ssl_ctx(ssl)            ├── take_ssl_ctx(ssl)
       ├── Layer 1 match              │   (消费快照)                   ├── 提取客户端证书
       ├── set_min_proto_version      ├── apply cert/mtls/ciphers      ├── 盖 handshake_complete_time
       └── store_ssl_ctx(ssl, ctx)    ├── finalize_ssl_ctx (tls_id)    └── log_ssl + 返回 Arc<TlsConnMeta>
                                      └── store_ssl_ctx(ssl, ctx)
                                              │
                                      (若握手被 abort，SSL 销毁时)
                                      free_ssl_ctx → tls_id.is_none()?
                                              → log_ssl "early handshake abort"
```

- `SSL_get_ex_new_index`：注册自定义 ex_data 索引（全局一次），同时注册 `free_ssl_ctx` 释放回调。
- `SSL_set_ex_data` / `store_ssl_ctx`：存储 `Box<SslCtx>` 指针（select_cb 和 cert_cb 各写一次）。
- `SSL_get_ex_data` / `take_ssl_ctx`：消费式取回并转移所有权（cert_cb 和 handshake_complete_cb 各取一次）。
- `free_ssl_ctx`：SSL 对象销毁时由 BoringSSL 调用，负责释放未被取回的 SslCtx，并在 `tls_id.is_none()`（`finalize_ssl_ctx` 未执行）时补发 `"early handshake abort"` 审计日志（见下节）。

非 BoringSSL 构建中，ex_data 操作为空操作，`handshake_complete_callback` 返回基础 `TlsConnMeta`。

## 早期 abort 审计（Task 10 引入）

当连接在 `select_certificate_callback` 之后、`certificate_callback` 之前就被 BoringSSL 终止（典型：min_tls_version 拒绝 TLS1.2 客户端的 alert protocol_version），常规 ssl.log 流程（cert_cb → finalize_ssl_ctx → handshake_complete_callback → log_ssl）不会触发。

为补全审计，`mod ssl_ctx_ex_data::free_ssl_ctx`（BoringSSL 通过 `SSL_get_ex_new_index` 注册的内存释放回调）在 SSL 对象销毁时检查 `tls_id.is_none()`——说明 `finalize_ssl_ctx` 没跑——则 push `"early handshake abort"` 到 err_log（不覆盖更具体的 err_log）并调用 `log_ssl`。

实现要点：
- 整个回调体包在 `std::panic::catch_unwind` 防止 panic 跨 FFI（与 `mtls_verify_callback.rs` 同样模式）。
- 仅当 `meta.err_log.is_none()` 时才设置 `"early handshake abort"`，保留 select_cb 之类上游已设置的更具体原因（如 `"min ver: ..."`）。
- 这条路径仅 boringssl 后端可用（依赖 ex_data 机制）。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | TlsCallback、SslCtx、证书应用 |
| `src/core/gateway/tls/store/cert_matcher.rs` | TlsCertMatcher（EdgionTls listener+SNI 匹配） |
| `src/core/gateway/runtime/matching/tls.rs` | GatewayTlsMatcher（Gateway 回退匹配） |
| `src/core/gateway/tls/runtime/backend/cert_extractor.rs` | 客户端证书信息提取 |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | BoringSSL mTLS 验证回调 |
