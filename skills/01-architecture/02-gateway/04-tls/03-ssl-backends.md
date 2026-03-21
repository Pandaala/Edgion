---
name: gateway-ssl-backends
description: SSL 后端实现：BoringSSL 和 OpenSSL 两种编译特性、接口抽象、选择建议。
---

# SSL 后端（BoringSSL / OpenSSL）

## 概述

Edgion Gateway 通过 Cargo feature flags 支持两种 SSL 后端。编译时选择其中一种，运行时行为通过条件编译 (`#[cfg(feature = "...")]`) 自动分派。

## Feature Flags

```toml
[features]
boringssl = [...]   # 使用 BoringSSL 作为 TLS 后端
openssl = [...]     # 使用 OpenSSL 作为 TLS 后端
```

两个特性互斥：在条件编译中使用 `#[cfg(all(feature = "openssl", not(feature = "boringssl")))]` 确保 BoringSSL 优先。

## BoringSSL 后端

```
src/core/gateway/tls/boringssl/
├── mod.rs
└── mtls_verify_callback.rs   # mTLS 自定义验证回调
```

### 功能支持

| 功能 | 支持状态 | 说明 |
|------|---------|------|
| TLS 终止 | 完整支持 | 证书/私钥设置、ALPN（HTTP/2） |
| mTLS 客户端验证 | 完整支持 | CA 验证、verify_depth |
| SAN/CN 白名单 | 完整支持 | 自定义 verify callback |
| SSL ex_data | 完整支持 | per-connection 上下文传递 |
| 最低 TLS 版本 | 完整支持 | TLS 1.0 ~ 1.3 |
| 密码套件配置 | 完整支持 | `SSL_set_strict_cipher_list` |
| TLS 1.3 密码套件 | 不可配置 | BoringSSL 硬编码 |

### mTLS 验证回调

`mtls_verify_callback.rs` 实现了自定义的证书验证回调，支持基于 SAN（Subject Alternative Name）和 CN（Common Name）的白名单验证。当 EdgionTls 配置了 `allowed_sans` 或 `allowed_cns` 时激活。

### ex_data 机制

BoringSSL 的 `SSL_set_ex_data` / `SSL_get_ex_data` 用于在 `certificate_callback` 和 `handshake_complete_callback` 之间传递 per-connection 上下文（`SslCtx`）。包含：

- `SSL_get_ex_new_index`：注册自定义索引（全局一次，`LazyLock` + `Once`）。
- 注册 `free_ssl_ctx` 回调处理 SSL 对象销毁时的内存释放。
- 线程安全：索引使用 `AtomicI32` 存储，初始化使用 `std::sync::Once`。

## OpenSSL 后端

```
src/core/gateway/tls/openssl/
└── mod.rs   # 预留模块（当前为空）
```

### 功能支持

| 功能 | 支持状态 | 说明 |
|------|---------|------|
| TLS 终止 | 基础支持 | 通过 Pingora 的 OpenSSL 绑定 |
| mTLS 客户端验证 | 基础支持 | 标准 OpenSSL 验证链 |
| SAN/CN 白名单 | 未实现 | 需要自定义 verify callback |
| SSL ex_data | 未实现 | 回调中返回基础 TlsConnMeta |
| 最低 TLS 版本 | 支持 | 通过 `set_min_proto_version` |
| 密码套件配置 | 无操作 | `configure_ciphers` 中被忽略 |

### 非 BoringSSL 回退

当编译为 OpenSSL 后端时：

- `store_ssl_ctx` 返回 `false`，记录 `"no boringssl exdata"` 日志。
- `take_ssl_ctx` 返回 `None`。
- `handshake_complete_callback` 返回包含基础信息的 `TlsConnMeta`（SNI、端口、时间戳）。
- `configure_ciphers` 中的 BoringSSL FFI 调用被条件编译移除。

## 统一接口

`backend_api.rs` 提供编译时分派的统一接口：

```rust
pub fn set_mtls_verify_callback(
    ssl: &mut SslRef,
    verify_mode: SslVerifyMode,
    edgion_tls: &Arc<EdgionTls>,
) -> Result<(), String>
```

- `#[cfg(feature = "boringssl")]`：调用 `boringssl::mtls_verify_callback::set_verify_callback_with_whitelist`。
- `#[cfg(all(feature = "openssl", not(feature = "boringssl")))]`：返回错误，提示未实现。

其他需要条件编译的操作直接在调用点使用 `#[cfg]` 属性，例如 `tls_pingora.rs` 中的 `configure_ciphers` 和 `verify_depth` 设置。

## 模块条件编译

`tls/runtime/backend/mod.rs` 的整个模块使用条件编译守卫：

```rust
#![cfg(any(feature = "boringssl", feature = "openssl"))]
```

确保在未启用任何 SSL 后端时不编译后端相关代码。

## 关键源文件

| 文件 | 职责 |
|------|------|
| `src/core/gateway/tls/mod.rs` | 模块入口、feature flag 条件编译 |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | BoringSSL mTLS 验证回调 |
| `src/core/gateway/tls/openssl/mod.rs` | OpenSSL 预留模块 |
| `src/core/gateway/tls/runtime/backend/backend_api.rs` | 统一后端 API |
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | 条件编译的 SSL ex_data 操作 |
