# Rustls mTLS 支持评估 — 基于 Pingora 0.8.0 变更分析

> 来源：Pingora 0.8.0 升级过程中对 mTLS 原生支持的深入调研（06-mtls-native-support）。
> 本文档记录调研结论，作为未来 Rustls 后端支持的参考依据。

## 背景

Pingora 0.8.0 release notes 提到：

> Add support for client certificate verification in mTLS configuration.

经源码 diff 验证，**此变更仅影响 Rustls 后端**，BoringSSL/OpenSSL 和 s2n 路径在 0.7.0 → 0.8.0 之间零变更。

## mTLS 方向

**Downstream（客户端 → 网关）**，即 listener 端验证连接进来的客户端证书。不涉及 upstream（网关 → 后端）。

## 变更内容

### Rustls 后端：从不可用到基本可用

**0.7.0** — 硬编码 `with_no_client_auth()`，完全无法验证客户端证书：

```rust
// pingora-core-0.7.0/src/listeners/tls/rustls/mod.rs L57-60
// TODO - Add support for client auth & custom CA support
let mut config = ServerConfig::builder_with_protocol_versions(&[&version::TLS12, &version::TLS13])
    .with_no_client_auth()
    .with_single_cert(certs, key)
```

**0.8.0** — 新增 `client_cert_verifier` 字段，支持注入自定义 `ClientCertVerifier`：

```rust
// pingora-core-0.8.0/src/listeners/tls/rustls/mod.rs L59-65
let builder = ServerConfig::builder_with_protocol_versions(&[&version::TLS12, &version::TLS13]);
let builder = if let Some(verifier) = self.client_cert_verifier {
    builder.with_client_cert_verifier(verifier)
} else {
    builder.with_no_client_auth()
};
```

新增 API：

```rust
// pingora-core-0.8.0/src/listeners/tls/rustls/mod.rs L93-96
pub fn set_client_cert_verifier(&mut self, verifier: Arc<dyn ClientCertVerifier>) {
    self.client_cert_verifier = Some(verifier);
}
```

### BoringSSL/OpenSSL 路径：零变更

`boringssl_openssl/mod.rs` 在 0.7.0 和 0.8.0 完全相同。Edgion 当前使用 `boringssl` feature，
走此路径，因此 **0.8.0 的 mTLS 变更对 Edgion 当前版本没有任何影响**。

## 对 Edgion 的意义

### 当前（BoringSSL）：无需操作

Edgion 的 mTLS 全部基于 BoringSSL 自定义实现，该路径 API 在 0.8.0 中无任何破坏性变更，
现有代码完全兼容。

### 未来（若支持 Rustls）：这是前置条件

0.8.0 的 `set_client_cert_verifier()` 是 Rustls 后端支持 mTLS 的**必要条件**。
但要完整移植 Edgion 的 mTLS 功能到 Rustls，还需解决以下差距：

## Edgion mTLS 功能 vs Rustls 路径能力对比

| Edgion 功能 | BoringSSL（当前） | Rustls（0.8.0） | 差距说明 |
|-------------|-------------------|-----------------|---------|
| CA 证书验证 | `ssl.set_verify_cert_store()` | `ClientCertVerifier` | ✅ 可通过 `WebPkiClientVerifier` 实现 |
| Mutual 模式 | `PEER \| FAIL_IF_NO_PEER_CERT` | verifier 决定 | ✅ 可实现 |
| OptionalMutual 模式 | `PEER` | verifier 决定 | ✅ 可通过 `allow_unauthenticated()` 实现 |
| verify_depth | `ssl.set_verify_depth()` | 未暴露 | ⚠️ 需在 `ClientCertVerifier` 中自行实现深度限制 |
| SAN/CN 白名单 | BoringSSL FFI verify callback | 不提供 | ⚠️ 需在自定义 `ClientCertVerifier` trait 中实现 |
| SAN 通配符匹配 | `matches_pattern()` | 不提供 | ⚠️ 验证逻辑可复用，但需包装在 `ClientCertVerifier` 中 |
| per-SNI+port 配置 | `certificate_callback` 按 SNI 分发 | **不支持** | ❌ Rustls 的 `with_callbacks()` 直接返回错误 |
| 动态证书加载 | `TlsAcceptCallbacks` | **不支持** | ❌ 同上，Rustls 不支持 certificate callbacks |
| 客户端证书信息透传 | `handshake_complete_callback` → `ClientCertInfo` | **不支持** | ❌ Rustls 无法在握手后将信息写入 `digest.extension` |

## Rustls mTLS 适配方案（未来参考）

如果未来决定支持 Rustls 后端，mTLS 适配需分两层：

### 第一层：可直接实现（依赖 0.8.0 API）

通过实现 `rustls::server::danger::ClientCertVerifier` trait：

```rust
use rustls::server::WebPkiClientVerifier;

// 基础 CA 验证
let roots = Arc::new(load_ca_roots(ca_pem)?);
let verifier = WebPkiClientVerifier::builder(roots)
    .build()?;

// 或自定义 verifier 实现 SAN/CN 白名单 + verify_depth
struct EdgionClientCertVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    allowed_sans: Option<Vec<String>>,
    allowed_cns: Option<Vec<String>>,
    verify_depth: u8,
}

impl ClientCertVerifier for EdgionClientCertVerifier {
    fn verify_client_cert(&self, ...) -> Result<...> {
        // 1. 调用 inner 做基础 CA 链验证
        // 2. 检查证书链深度
        // 3. 提取 SAN/CN 做白名单匹配（复用现有 mtls.rs 逻辑）
    }
}
```

### 第二层：受阻于 Pingora Rustls 限制

以下功能需要等 Pingora 上游支持或 Edgion 自行适配：

1. **动态证书 + per-SNI 配置** — Rustls 的 `with_callbacks()` 当前返回错误。
   需要 Pingora 上游为 Rustls 实现 `TlsAcceptCallbacks`，或 Edgion 使用 rustls 的
   `ResolvesServerCert` trait 自行实现 SNI 路由。

2. **客户端证书信息透传** — 无 `handshake_complete_callback`。
   需要在请求处理层（如 `request_filter`）通过 rustls 的 `ServerConnection::peer_certificates()`
   提取证书信息，而非在握手回调中完成。

## 行动项

- [ ] 持续关注 Pingora 后续版本是否为 Rustls 补上 `TlsAcceptCallbacks` 支持
- [ ] 若启动 Rustls 后端支持项目，优先评估 SNI 动态路由方案（`ResolvesServerCert`）
- [ ] 评估是否将 `mtls.rs` 中的验证逻辑抽象为 TLS-backend-agnostic 的公共模块
- [ ] 评估 rustls `ServerConnection::peer_certificates()` 作为证书信息透传的替代方案

## 相关文件

| 文件 | 说明 |
|------|------|
| `tasks/working/pingora-0.8.0-upgrade/06-mtls-native-support.md` | 原始 task（已完成，无需操作） |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | BoringSSL FFI verify callback |
| `src/core/gateway/tls/validation/mtls.rs` | SAN/CN 白名单验证（可复用） |
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | mTLS 配置入口 |
| `src/types/resources/edgion_tls.rs` | `ClientAuthConfig` 配置结构 |
| `src/types/ctx.rs` | `ClientCertInfo` 定义 |
