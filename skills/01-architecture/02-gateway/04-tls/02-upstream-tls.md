---
name: gateway-upstream-tls
description: 上游 TLS（网关→后端）：mTLS 配置、BackendTLSPolicy、证书提取。
---

# 上游 TLS（网关→后端）

## 概述

上游 TLS 处理 Gateway 到后端服务的 TLS 连接。当后端要求 TLS 时，Gateway 作为 TLS 客户端与后端建立安全连接。配置由 Gateway API 的 `BackendTLSPolicy` 资源驱动。

## 模块结构

```
src/core/gateway/tls/runtime/backend/
├── mod.rs               # 模块入口，条件编译 (#[cfg(any(feature = "boringssl", feature = "openssl"))])
├── backend_api.rs       # 统一后端 API（BoringSSL/OpenSSL 分派）
└── cert_extractor.rs    # 客户端证书信息提取工具
```

## BackendTLSPolicy

`BackendTLSPolicy` 是 Gateway API 标准资源，定义了 Gateway 到特定后端服务的 TLS 策略。在路由匹配完成、后端选择后，`pg_upstream_peer.rs` 查询该策略来决定上游连接的 TLS 配置。

核心配置项：

- **目标后端**：通过 `targetRef` 指定要启用 TLS 的 Service。
- **验证配置**：CA 证书、主机名验证。
- **mTLS**：Gateway 作为客户端提供证书给后端。

BackendTLSPolicy 查询发生在 `pg_upstream_peer.rs` 中，此时路由命名空间信息可用，能正确处理命名空间继承。

## mTLS 到后端

当 BackendTLSPolicy 要求 mTLS 时，Gateway 需要同时：

1. 验证后端的服务端证书。
2. 向后端出示自己的客户端证书。

### 验证回调设置

`backend_api.rs` 提供统一的 `set_mtls_verify_callback` 函数，根据编译特性分派到具体后端：

```rust
pub fn set_mtls_verify_callback(
    ssl: &mut SslRef,
    verify_mode: SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String>
```

- **BoringSSL**：调用 `boringssl::mtls_verify_callback::set_verify_callback_with_whitelist`，支持完整的 SAN/CN 白名单验证。
- **OpenSSL**：当前未实现，返回错误信息。

## 证书信息提取

`cert_extractor.rs` 提供 `extract_client_cert_info` 函数，从 SSL 连接中提取对端证书的详细信息：

```rust
pub fn extract_client_cert_info(ssl: &SslRef) -> Option<ClientCertInfo>
```

提取的信息：

| 字段 | 说明 |
|------|------|
| `subject` | 证书 Subject DN，格式为 `CN=xxx, O=yyy` |
| `cn` | Common Name |
| `sans` | Subject Alternative Names 列表 |
| `fingerprint` | SHA256 指纹，格式为 `xx:xx:xx:...` |

### SAN 提取

支持的 SAN 类型：

- **DNS**：直接提取为字符串。
- **IP**：解析二进制数据为 IPv4（4 字节）或 IPv6（16 字节，使用 `std::net::Ipv6Addr` 格式化），前缀 `IP:`。
- **Email**：前缀 `email:`。
- **URI**：前缀 `uri:`。

该函数主要用于下游 mTLS 场景（`handshake_complete_callback` 中提取客户端证书），但同样可用于上游连接的证书检查。

## 证书验证

`tls/validation/` 模块提供证书有效性验证：

- `validate_cert`：验证 EdgionTls 资源的证书是否有效（PEM 格式、私钥匹配等）。
- `CertValidationResult`：验证结果，包含 `is_valid` 标记和 `errors` 列表。
- 验证在证书写入 TlsStore 时同步执行，无效证书不进入匹配器。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/tls/runtime/backend/backend_api.rs` | 统一后端 API、set_mtls_verify_callback |
| `src/core/gateway/tls/runtime/backend/cert_extractor.rs` | 证书信息提取（subject/SAN/fingerprint） |
| `src/core/gateway/tls/validation/cert.rs` | 证书有效性验证 |
| `src/core/gateway/tls/validation/mtls.rs` | mTLS 相关验证 |
| `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` | BackendTLSPolicy 查询与上游 TLS 连接 |
