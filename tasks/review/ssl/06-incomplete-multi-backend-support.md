# SSL/TLS 审查报告 #06：多后端 TLS 支持不完整（OpenSSL/rustls 缺口）

**审查日期**：2026-03-30  
**严重级别**：高  
**影响范围**：使用 `openssl` 或 `rustls` feature 编译时，TLS 相关功能大面积失效  
**状态**：待修复

---

## 一、问题概述

Edgion 项目的 TLS 层设计上宣称支持三种后端——BoringSSL、OpenSSL、rustls（在 `Cargo.toml` 中均已定义为互斥的 feature），但实际实现几乎完全围绕 BoringSSL 构建。OpenSSL 后端仅有空壳模块，rustls 后端在大量关键路径中被 `#[cfg]` 门直接排除，既无实现也无优雅降级。

这意味着：**当用户选择 `--features openssl` 或 `--features rustls` 编译时，网关的核心 TLS 功能——从证书加载、mTLS 认证到密码套件配置——要么返回硬编码错误，要么静默跳过，要么直接编译失败。**

---

## 二、功能矩阵：后端实现覆盖度

| 功能点 | BoringSSL | OpenSSL | rustls |
|--------|:---------:|:-------:|:------:|
| **TLS 监听器创建**（HTTPS/TLS Terminate） | ✅ 完整 | ⚠️ 编译通过但运行时功能缺失 | ❌ 编译被 `#[cfg]` 排除 |
| **SSL ex_data 上下文传递**（证书回调 → 握手完成回调） | ✅ 完整 | ❌ 始终返回 `false`/`None` | ❌ 同 OpenSSL fallback |
| **运行时证书过期检查** | ✅ 完整（`Asn1Time` 比较） | ❌ 跳过（依赖握手层拒绝） | ❌ 跳过 |
| **mTLS SAN/CN 白名单回调** | ✅ 完整（自定义 verify callback） | ❌ 返回硬编码错误 | ❌ 无 `#[cfg]` 分支，编译可能失败 |
| **InsecureFallback mTLS 模式** | ✅ 完整（自定义 verify callback 始终返回 1） | ⚠️ 降级为 `SslVerifyMode::PEER`（语义不同） | ⚠️ 同 OpenSSL fallback |
| **set_verify_depth（mTLS 验证深度）** | ✅ 完整 | ❌ 静默忽略（`let _ = verify_depth`） | ❌ 同 OpenSSL fallback |
| **configure_ciphers（密码套件配置）** | ✅ 完整（FFI 调用 `SSL_set_strict_cipher_list`） | ❌ 静默跳过（no-op） | ❌ 同 OpenSSL fallback |
| **peek_tls_conn_meta**（mTLS 回调日志） | ✅ 完整 | ❌ 未导出 | ❌ 未导出 |
| **OpenSSL 专属模块** | N/A | ❌ 空文件 | N/A |
| **客户端证书提取**（`cert_extractor`） | ✅ 完整 | ⚠️ 依赖 pingora 的 OpenSSL 绑定（可能可用） | ❓ 取决于 pingora-rustls API |

**图例**：✅ = 完整实现，⚠️ = 部分可用/语义降级，❌ = 不可用/跳过/失败，❓ = 未验证

---

## 三、逐文件分析

### 3.1 `src/core/gateway/tls/runtime/backend/backend_api.rs`

**问题**：`set_mtls_verify_callback()` 缺少 rustls 分支

```rust
// 第 29-43 行
#[cfg(feature = "boringssl")]
{
    // BoringSSL: 完整实现
}

#[cfg(all(feature = "openssl", not(feature = "boringssl")))]
{
    // OpenSSL: 直接返回 Err
    Err("mTLS SAN/CN whitelist validation not yet implemented for OpenSSL backend".to_string())
}
// ⚠️ 无 #[cfg(feature = "rustls")] 分支
```

**影响**：
- **OpenSSL**：当 `EdgionTls` 配置了 `allowed_sans` 或 `allowed_cns` 时，mTLS 配置会直接报错，连接拒绝建立。
- **rustls**：由于 `runtime/mod.rs` 整个模块被 `#[cfg(any(feature = "boringssl", feature = "openssl"))]` 守护，rustls 根本不会编译到此函数。但如果未来有人调整了 cfg 门，函数将无返回值（编译错误）。

### 3.2 `src/core/gateway/tls/openssl/mod.rs`

**问题**：空壳模块

```rust
#![cfg(feature = "openssl")]
// Reserved for OpenSSL-specific functionality
```

**影响**：整个文件仅有一个内部 cfg 属性和一行注释。对比 `boringssl/mod.rs`（导出了 `mtls_verify_callback` 子模块），OpenSSL 后端没有任何对等实现。这表明 OpenSSL 支持从未真正启动开发。

### 3.3 `src/core/gateway/tls/runtime/gateway/tls_pingora.rs`

此文件是 TLS 运行时的核心，包含了最多的 BoringSSL 特定代码。以下逐一分析各问题点：

#### 3.3.1 `ssl_ctx_ex_data`（第 67-201 行）—— 连接上下文传递完全失效

**BoringSSL 实现**（第 67-179 行）：
通过 `SSL_get_ex_new_index` / `SSL_set_ex_data` / `SSL_get_ex_data` FFI 调用，在 SSL 对象上挂载自定义 `SslCtx` 结构体，实现 `certificate_callback` → `handshake_complete_callback` 的数据传递。

**非 BoringSSL fallback**（第 181-201 行）：
```rust
pub(super) fn store_ssl_ctx(_ssl: &mut SslRef, ctx: &SslCtx) -> bool {
    // 记录日志 "no boringssl exdata"
    false  // 始终返回失败
}

pub(super) fn take_ssl_ctx(_ssl: &TlsRef) -> Option<SslCtx> {
    None  // 始终返回 None
}
```

**影响链**：
1. `certificate_callback`（第 263 行）调用 `store_ssl_ctx` → 返回 `false` → 日志记录 "store ctx failed"
2. `handshake_complete_callback`（第 273 行）调用 `take_ssl_ctx` → 返回 `None` → 走 fallback 路径（第 290-314 行）
3. Fallback 路径创建的 `TlsConnMeta` 缺少：
   - `tls_id`：无法追踪连接
   - `matched`：无法知道匹配了哪个证书资源
   - `client_cert_info`：mTLS 客户端证书信息丢失
   - `is_mtls`：始终为 `false`

**根本原因**：OpenSSL 同样提供 `SSL_get_ex_new_index` / `SSL_set_ex_data` API（接口几乎相同），但此处未做适配。rustls 虽然没有 FFI 级别的 ex_data，但可通过 `Arc<Mutex<>>` 或 `thread_local!` 等 Rust 原生方式实现等效功能。

#### 3.3.2 运行时证书过期检查（第 413-438 行）—— 仅 BoringSSL

```rust
#[cfg(feature = "boringssl")]
match boring::asn1::Asn1Time::days_from_now(0) {
    Ok(now) => {
        if cert.not_after() < &now { /* 拒绝过期证书 */ }
        if cert.not_before() > &now { /* 拒绝尚未生效的证书 */ }
    }
    Err(e) => { /* 记录警告，允许继续 */ }
}
```

**影响**：OpenSSL/rustls 编译下，已过期或尚未生效的证书不会被运行时检测到。虽然注释说"依赖握手层拒绝"，但这有两个问题：
1. 握手层拒绝是 TLS 协议行为，取决于对端（客户端）是否校验——网关作为服务端使用过期证书发起握手，很多客户端会拒绝，但错误信息不会出现在 Edgion 的 `err_log` 中，增加排障难度。
2. `not_before` 检查（证书尚未生效）在握手层不一定会被拒绝——部分 TLS 库/客户端不检查这个字段。

#### 3.3.3 `configure_ciphers`（第 804-838 行）—— 密码套件配置为空操作

```rust
#[cfg(feature = "boringssl")]
{
    // FFI 调用 SSL_set_strict_cipher_list
}

#[cfg(not(feature = "boringssl"))]
{
    let _ = ssl;  // 直接丢弃，什么都不做
}
```

**影响**：用户在 `EdgionTls` 资源中配置了 `spec.ciphers`，但在 OpenSSL/rustls 下完全被忽略。没有任何警告日志，配置静默失效。这属于安全相关的配置丢失——用户认为限制了密码套件，实际上并未生效。

#### 3.3.4 `configure_gateway_frontend_mtls` — InsecureFallback 降级（第 664-688 行）

```rust
if mode == "AllowInsecureFallback" {
    #[cfg(feature = "boringssl")]
    unsafe {
        // 自定义 verify callback 始终返回 1（允许所有证书）
    }
    #[cfg(not(feature = "boringssl"))]
    {
        ssl.set_verify(SslVerifyMode::PEER);
        // ⚠️ PEER 模式会拒绝无效证书，与 InsecureFallback 语义不符
    }
}
```

**影响**：`AllowInsecureFallback` 模式的语义是"请求客户端证书，但即使无效/缺失也允许连接"。非 BoringSSL fallback 使用 `SslVerifyMode::PEER`，这会导致无效证书被拒绝——行为与配置意图**完全相反**。这是一个**语义正确性 bug**。

#### 3.3.5 `set_verify_depth`（第 747-754 行，第 693-696 行）—— 静默忽略验证深度

两处 `set_verify_depth` 调用均仅在 BoringSSL 下生效：

```rust
// configure_mtls 中（第 747-754 行）
#[cfg(feature = "boringssl")]
{ ssl.set_verify_depth(client_auth.verify_depth as u32); }
#[cfg(not(feature = "boringssl"))]
{ let _ = client_auth.verify_depth; }  // 丢弃

// configure_gateway_frontend_mtls 中（第 693-696 行）
#[cfg(feature = "boringssl")]
{ ssl.set_verify_depth(10); }
// 非 boringssl：没有任何代码
```

**影响**：OpenSSL/rustls 下，mTLS 证书链验证深度不受控制，使用 TLS 库的默认值。如果默认值过大，可能接受深度过长的证书链，构成安全隐患。

### 3.4 `src/core/gateway/runtime/server/listener_builder.rs`

**问题**：rustls 被完全排除在 TLS 监听器之外

关键的 `#[cfg]` 门：

| 行号 | cfg 条件 | 影响 |
|------|---------|------|
| 26 | `#[cfg(any(feature = "boringssl", feature = "openssl"))]` | `TlsCallback` 的 import 排除 rustls |
| 219 | `#[cfg(any(feature = "boringssl", feature = "openssl"))]` | HTTPS 监听器创建排除 rustls |
| 247 | `#[cfg(not(any(feature = "boringssl", feature = "openssl")))]` | rustls 下直接 bail 错误 |
| 353 | `#[cfg(any(feature = "boringssl", feature = "openssl"))]` | `add_tls_terminate_to_tcp_listener` 整个函数排除 rustls |
| 477 | `#[cfg(any(feature = "boringssl", feature = "openssl"))]` | TLS Terminate 模式排除 rustls |

**影响**：使用 `--features rustls` 编译后，所有 HTTPS 和 TLS 监听器创建都会返回 `"TLS support requires either 'boringssl' or 'openssl' feature"` 错误。网关将无法提供任何 TLS 服务。

### 3.5 `src/core/gateway/tls/runtime/mod.rs` 和 `backend/mod.rs`

```rust
// runtime/mod.rs
#[cfg(any(feature = "boringssl", feature = "openssl"))]
pub mod backend;
#[cfg(any(feature = "boringssl", feature = "openssl"))]
pub mod gateway;

// backend/mod.rs
#![cfg(any(feature = "boringssl", feature = "openssl"))]
```

rustls 被排除在整个 TLS 运行时模块树之外。

---

## 四、影响分析

### 4.1 使用 `--features openssl` 编译

| 场景 | 结果 |
|------|------|
| 基本 HTTPS 服务（无 mTLS） | ⚠️ 可启动，但：<br>- 连接上下文丢失（无 tls_id，追踪失效）<br>- 证书过期检查缺失<br>- 密码套件配置被忽略 |
| EdgionTls mTLS（带 SAN/CN 白名单） | ❌ 报错："mTLS SAN/CN whitelist validation not yet implemented" |
| EdgionTls mTLS（仅 CA 验证） | ⚠️ 可工作，但 verify_depth 无效 |
| Gateway frontendValidation mTLS | ⚠️ 可工作，但：<br>- verify_depth 无效<br>- InsecureFallback 模式语义错误 |
| 密码套件限制 | ❌ 配置静默失效 |
| SSL 日志与可观测性 | ❌ 严重降级（缺少匹配信息、tls_id、客户端证书信息） |

### 4.2 使用 `--features rustls` 编译

| 场景 | 结果 |
|------|------|
| 任何 TLS 相关功能 | ❌ 完全不可用。所有 TLS 监听器返回编译时错误或运行时 bail。 |
| 非 TLS 功能（HTTP、TCP、UDP） | ✅ 正常工作 |

### 4.3 安全风险总结

1. **InsecureFallback 语义错误**（高）：非 BoringSSL 下 AllowInsecureFallback 实际拒绝无效证书，与用户预期相反。
2. **密码套件配置静默失效**（高）：安全加固配置不生效，用户无感知。
3. **verify_depth 不受控**（中）：证书链深度验证缺失，可能被利用。
4. **证书过期检查缺失**（中）：过期证书可能被用于服务，直到客户端拒绝。

---

## 五、根因分析

### 5.1 直接原因

项目从 BoringSSL 起步，后续在 `Cargo.toml` 中定义了 OpenSSL 和 rustls feature，但未配套实现。`#[cfg]` 分支主要作为"占位符"添加，fallback 实现或为空操作或为硬编码失败。

### 5.2 架构原因

**缺少后端抽象层**。当前代码直接在业务逻辑中通过 `#[cfg]` 条件编译来区分后端，导致：
- 每个需要 TLS 操作的函数都需要手动添加 3 个 `#[cfg]` 分支
- 新增功能时容易遗漏非默认后端的分支
- 没有编译时保障确保所有后端实现了同一组接口

对比理想架构：

```
当前结构（分散的 #[cfg]）：
  tls_pingora.rs → #[cfg(boringssl)] { ... } #[cfg(not(boringssl))] { no-op }
  backend_api.rs → #[cfg(boringssl)] { ... } #[cfg(openssl)] { Err }
  listener_builder.rs → #[cfg(boringssl || openssl)] { ... }

理想结构（抽象 trait）：
  trait TlsBackend {
      fn store_connection_context(ssl, ctx) -> bool;
      fn take_connection_context(ssl) -> Option<Ctx>;
      fn set_verify_callback(ssl, mode, tls) -> Result<()>;
      fn set_verify_depth(ssl, depth);
      fn configure_ciphers(ssl, ciphers) -> Result<()>;
      fn check_cert_expiry(cert) -> Result<()>;
      fn set_insecure_fallback_verify(ssl);
  }
```

### 5.3 流程原因

- 缺少 feature 覆盖的 CI 矩阵——可能只有 `boringssl` 在 CI 中被测试
- 缺少"feature parity"跟踪机制——没有文档或测试记录各后端应实现哪些能力

---

## 六、修复策略建议

### 6.1 短期：明确标记不支持的后端（1-2 天）

**目标**：消除"静默失效"问题，让用户明确知道哪些功能不可用。

1. **对所有 no-op fallback 添加 `tracing::warn!` 日志**：
   - `configure_ciphers` 非 BoringSSL 路径
   - `set_verify_depth` 非 BoringSSL 路径
   - 证书过期检查跳过路径

2. **修复 InsecureFallback 语义 bug**（安全修复，优先级最高）：
   - OpenSSL 后端可使用 `openssl-sys::SSL_set_verify` 实现自定义回调
   - 如暂时无法实现，应拒绝使用 InsecureFallback 并返回明确错误，而非静默降级为不同行为

3. **在文档/错误信息中明确后端支持状态**：
   - 启动时检测当前 TLS 后端，打印功能限制警告
   - 在 `EdgionGatewayConfig` 验证层拒绝不支持的配置组合

### 6.2 中期：补齐 OpenSSL 后端（1-2 周）

**目标**：OpenSSL 达到与 BoringSSL 功能对等。

按优先级排序：

| 优先级 | 任务 | 复杂度 | 说明 |
|-------|------|--------|------|
| P0 | 修复 InsecureFallback 语义 | 低 | OpenSSL FFI 调用 `SSL_set_verify` |
| P1 | 实现 `ssl_ctx_ex_data`（OpenSSL） | 中 | OpenSSL 提供相同的 `SSL_get_ex_new_index` API |
| P1 | 实现 `set_verify_depth`（OpenSSL） | 低 | `SslRef::set_verify_depth` 在 openssl crate 中可用 |
| P1 | 实现 `configure_ciphers`（OpenSSL） | 低 | `SslRef::set_cipher_list` 在 openssl crate 中可用 |
| P2 | 实现运行时证书过期检查（OpenSSL） | 低 | 使用 `openssl::asn1::Asn1Time` |
| P2 | 实现 mTLS SAN/CN 白名单回调（OpenSSL） | 中 | 需要 FFI 自定义 verify callback |
| P3 | 填充 `openssl/mod.rs` 模块 | - | 随上述任务自然完成 |

### 6.3 长期：引入 TLS 后端抽象层 + rustls 支持（2-4 周）

**目标**：建立可扩展的多后端架构。

1. **定义 `TlsBackendOps` trait**：

```rust
pub trait TlsBackendOps {
    type SslHandle;
    type CertHandle;

    fn store_connection_context(ssl: &mut Self::SslHandle, ctx: ConnectionCtx) -> bool;
    fn take_connection_context(ssl: &Self::SslHandle) -> Option<ConnectionCtx>;
    fn set_mtls_verify_callback(ssl: &mut Self::SslHandle, mode: VerifyMode, config: &MtlsConfig) -> Result<()>;
    fn set_verify_depth(ssl: &mut Self::SslHandle, depth: u32);
    fn configure_ciphers(ssl: &mut Self::SslHandle, ciphers: &[String]) -> Result<()>;
    fn check_cert_validity(cert: &Self::CertHandle) -> Result<CertValidity>;
    fn set_insecure_verify(ssl: &mut Self::SslHandle);
}
```

2. **为每个后端实现 trait**，移除业务代码中的 `#[cfg]` 分支。

3. **rustls 适配**：
   - `ssl_ctx_ex_data` → 使用 `Arc<Mutex<HashMap<ConnectionId, SslCtx>>>` 或 rustls `ServerConfig` 的 user_data
   - 证书操作 → 使用 `rustls::Certificate` + `x509-parser` crate
   - 自定义 verify callback → 实现 `rustls::server::danger::ClientCertVerifier`
   - 密码套件 → `rustls::CipherSuite` 配置

4. **CI feature 矩阵**：
   - 在 CI 中分别对 `boringssl`、`openssl`、`rustls` 三个 feature 编译并运行测试
   - 添加集成测试验证各后端的 TLS 功能

### 6.4 守护措施

1. **编译时保障**：通过 trait + 泛型确保新增 TLS 功能必须为所有后端提供实现
2. **Feature parity 测试**：创建参数化测试（`#[cfg(any(...))]`），确保相同的测试用例在所有后端下通过
3. **CI 矩阵覆盖**：`cargo build --features boringssl`、`cargo build --features openssl`、`cargo build --features rustls` 必须全部通过

---

## 七、实现优先级总结

```
P0（立即）  ─── 修复 InsecureFallback 语义 bug（安全问题）
P0（立即）  ─── 为静默失效的配置项添加 warn 日志
P1（1 周内）─── OpenSSL ssl_ctx_ex_data 实现
P1（1 周内）─── OpenSSL set_verify_depth / configure_ciphers 实现
P2（2 周内）─── OpenSSL 运行时证书过期检查
P2（2 周内）─── OpenSSL mTLS SAN/CN 白名单回调
P3（规划中）─── TlsBackendOps 抽象 trait 设计与重构
P3（规划中）─── rustls 后端完整实现
P3（规划中）─── CI feature 矩阵与 parity 测试
```

---

## 八、受影响文件清单

| 文件 | 问题类型 | 影响后端 |
|------|---------|---------|
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | ex_data 失效、多处 no-op fallback、语义错误 | OpenSSL, rustls |
| `src/core/gateway/tls/runtime/backend/backend_api.rs` | 缺少 rustls 分支、OpenSSL 硬编码错误 | OpenSSL, rustls |
| `src/core/gateway/tls/openssl/mod.rs` | 空壳模块 | OpenSSL |
| `src/core/gateway/runtime/server/listener_builder.rs` | rustls 被 cfg 排除 | rustls |
| `src/core/gateway/tls/runtime/mod.rs` | rustls 被 cfg 排除 | rustls |
| `src/core/gateway/tls/runtime/backend/mod.rs` | rustls 被 cfg 排除 | rustls |
| `Cargo.toml` | 定义了 rustls feature 但无实际支持 | rustls |
