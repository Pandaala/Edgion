# SSL Review: 证书与私钥匹配校验缺失

| 属性       | 值                                                                   |
|----------|----------------------------------------------------------------------|
| **严重程度** | **高 (High)** — 安全性缺陷 + 运维体验严重劣化                                     |
| **影响范围** | 所有通过 EdgionTls 加载的 TLS 证书                                            |
| **影响文件** | `src/core/gateway/tls/validation/cert.rs` (主文件)                       |
| **关联文件** | `src/core/gateway/tls/store/tls_store.rs`、`src/core/gateway/tls/runtime/gateway/tls_pingora.rs` |
| **状态**   | 待修复                                                                  |

---

## 1. 问题描述

`check_key_valid()` 函数（`cert.rs` 第 320-338 行）**仅验证了私钥 PEM 能否被解析**，但完全没有校验私钥是否与证书的公钥匹配。代码中通过注释明确承认了这一限制：

```319:338:src/core/gateway/tls/validation/cert.rs
/// Check if private key is valid
fn check_key_valid(key_pem: &str) -> Result<(), CertValidationError> {
    // Parse private key PEM
    let key_pem_obj = Pem::iter_from_buffer(key_pem.as_bytes())
        .next()
        .ok_or_else(|| CertValidationError::KeyParseError("No PEM block found".to_string()))?
        .map_err(|e| CertValidationError::KeyParseError(format!("PEM parse error: {}", e)))?;

    // Basic validation: check if key PEM is valid
    if key_pem_obj.contents.is_empty() {
        return Err(CertValidationError::KeyParseError("Empty key content".to_string()));
    }

    // Note: Full key-certificate matching requires cryptographic operations
    // which are complex and depend on the key type (RSA, ECDSA, etc.)
    // For now, we just validate that the key can be parsed
    // A more complete implementation would verify the public key matches

    Ok(())
}
```

更关键的问题是：**该函数的签名 `fn check_key_valid(key_pem: &str)` 只接收私钥参数，根本没有接收证书信息**——即使想做匹配，当前函数签名也无法实现。

同时，调用处（第 166-170 行）只传入了 `key_pem`，而上方已经解析好的证书 DER 数据 `der_data` 未被传递：

```165:170:src/core/gateway/tls/validation/cert.rs
    // 7. Check key matches certificate (if key exists)
    if let Ok(key_pem) = tls.key_pem() {
        if let Err(e) = check_key_valid(&key_pem) {
            errors.push(e);
        }
    }
```

此外，错误枚举 `CertValidationError::KeyMismatch` 已经定义（第 56 行）但**从未被使用**——这是一段死代码，进一步印证了该功能从未被实现。

---

## 2. 影响分析

### 2.1 直接影响：配置时无法发现错配

当前验证流水线在 TlsStore 写入时调用 `validate_cert()`（`tls_store.rs` 第 56、127、149 行）。如果用户错误地将 A 域名的私钥配给了 B 域名的证书，验证结果将显示为 **valid**，证书会被加入 `TlsCertMatcher`，对外提供服务。

### 2.2 运行时影响：TLS 握手失败，错误信息晦涩

在 `tls_pingora.rs` 的 `apply_edgion_tls_cert()` 方法中（第 382-486 行），证书和私钥分别通过 `ssl_use_certificate` 和 `ssl_use_private_key` 设置到 SSL 上下文。如果两者不匹配：

- **BoringSSL/OpenSSL** 会在 `ssl_use_private_key` 调用时返回错误（如 `error:0B080074:x509 certificate routines:X509_check_private_key:key values mismatch`），运行时日志中记录为 `"et: set key: ..."` 这样的短日志。
- 每次客户端连接都会重复触发此错误，产生大量无用日志。
- 用户从外部看到的是 TLS 握手失败，无法建立连接，但 **无法从 Edgion 的配置验证层得到任何有意义的提示**。

### 2.3 运维体验影响

| 场景 | 当前行为 | 期望行为 |
|------|---------|---------|
| 证书轮换时误用了另一张证书的私钥 | 配置写入成功，证书标记为 valid，运行时每次连接报错 | 配置写入时立即报 `KeyMismatch`，证书标记为 invalid，不进入匹配器 |
| 通过 Kubernetes Secret 更新证书，cert 和 key 来自不同来源 | 静默通过验证，运行时故障 | 在 controller 同步阶段即可发现问题 |
| ACME 自动签发证书后替换 | 如果旧 key 未同步更新，新 cert 配旧 key 静默通过 | 立即告警 |

### 2.4 安全性影响

- **低安全风险**：错配的 cert/key 不会导致数据泄露（因为握手会失败），但构成 **拒绝服务（DoS）向量**——攻击者如果能影响 Secret 内容，可以通过注入不匹配的 key 使特定域名的 TLS 服务完全瘫痪，且难以从配置层面排查。

---

## 3. 根因分析

1. **函数签名设计缺陷**：`check_key_valid` 只接收 `key_pem`，不接收证书信息，架构上就不可能完成匹配。
2. **验证流水线断裂**：`validate_cert()` 中第 147 行已经解析出 `der_data`（证书 DER 数据），但在第 167 行调用 `check_key_valid` 时没有将其传入。
3. **跨算法匹配的复杂性**：注释提到"depends on key type (RSA, ECDSA, etc.)"——纯用 `x509-parser` 做 key matching 确实需要分别处理不同密钥类型。但项目已经依赖 `boring` crate（BoringSSL 绑定），可以直接利用其 `public_eq()` API，无需手动处理密钥类型。

---

## 4. 推荐修复方案

### 4.1 核心思路

利用项目已有的 `boring` crate（boringssl feature，默认启用）提供的 `PKey::public_eq()` 方法，将证书的公钥与私钥进行比对。对于非 boringssl 后端（openssl / rustls），提供条件编译的替代实现。

### 4.2 修改点概览

**文件：`src/core/gateway/tls/validation/cert.rs`**

#### 步骤 1：修改函数签名，传入证书 PEM

将 `check_key_valid(key_pem)` 改为 `check_key_matches_cert(key_pem, cert_pem)`：

```rust
/// Validate private key and verify it matches the certificate's public key.
fn check_key_matches_cert(key_pem: &str, cert_pem: &str) -> Result<(), CertValidationError> {
    // Step 1: Validate key PEM format
    let key_pem_obj = Pem::iter_from_buffer(key_pem.as_bytes())
        .next()
        .ok_or_else(|| CertValidationError::KeyParseError("No PEM block found".to_string()))?
        .map_err(|e| CertValidationError::KeyParseError(format!("PEM parse error: {}", e)))?;

    if key_pem_obj.contents.is_empty() {
        return Err(CertValidationError::KeyParseError(
            "Empty key content".to_string(),
        ));
    }

    // Step 2: Verify private key matches certificate public key
    verify_key_cert_match(key_pem, cert_pem)
}
```

#### 步骤 2：实现基于 boring crate 的匹配逻辑（条件编译）

```rust
#[cfg(feature = "boringssl")]
fn verify_key_cert_match(key_pem: &str, cert_pem: &str) -> Result<(), CertValidationError> {
    let cert = boring::x509::X509::from_pem(cert_pem.as_bytes()).map_err(|e| {
        CertValidationError::CertParseError(format!("Failed to parse cert for key matching: {}", e))
    })?;

    let private_key = boring::pkey::PKey::private_key_from_pem(key_pem.as_bytes()).map_err(|e| {
        CertValidationError::KeyParseError(format!("Failed to parse private key: {}", e))
    })?;

    let cert_pubkey = cert.public_key().map_err(|e| {
        CertValidationError::KeyMismatch(format!("Failed to extract public key from cert: {}", e))
    })?;

    if cert_pubkey.public_eq(&private_key) {
        Ok(())
    } else {
        Err(CertValidationError::KeyMismatch(
            "Private key does not match certificate public key".to_string(),
        ))
    }
}

#[cfg(all(feature = "openssl", not(feature = "boringssl")))]
fn verify_key_cert_match(key_pem: &str, cert_pem: &str) -> Result<(), CertValidationError> {
    // openssl crate 具有与 boring 相同的 API
    let cert = openssl::x509::X509::from_pem(cert_pem.as_bytes()).map_err(|e| {
        CertValidationError::CertParseError(format!("Failed to parse cert for key matching: {}", e))
    })?;

    let private_key = openssl::pkey::PKey::private_key_from_pem(key_pem.as_bytes()).map_err(|e| {
        CertValidationError::KeyParseError(format!("Failed to parse private key: {}", e))
    })?;

    let cert_pubkey = cert.public_key().map_err(|e| {
        CertValidationError::KeyMismatch(format!("Failed to extract public key from cert: {}", e))
    })?;

    if cert_pubkey.public_eq(&private_key) {
        Ok(())
    } else {
        Err(CertValidationError::KeyMismatch(
            "Private key does not match certificate public key".to_string(),
        ))
    }
}

#[cfg(all(feature = "rustls", not(feature = "boringssl"), not(feature = "openssl")))]
fn verify_key_cert_match(key_pem: &str, cert_pem: &str) -> Result<(), CertValidationError> {
    use x509_parser::prelude::*;

    // 对于 rustls 后端，通过 x509-parser 提取证书的 SPKI（SubjectPublicKeyInfo）
    // 并与从私钥派生的公钥进行字节级比较。
    //
    // 这里采用 "试签名再验证" 策略：用私钥签署一段随机数据，
    // 再用证书公钥验证签名，以此确认两者匹配。
    let cert_pem_obj = Pem::iter_from_buffer(cert_pem.as_bytes())
        .next()
        .ok_or_else(|| CertValidationError::CertParseError("No cert PEM for matching".to_string()))?
        .map_err(|e| CertValidationError::CertParseError(format!("Cert PEM parse: {}", e)))?;

    let (_, cert) = X509Certificate::from_der(&cert_pem_obj.contents)
        .map_err(|e| CertValidationError::CertParseError(format!("Cert DER parse: {}", e)))?;

    let spki = cert.public_key();

    // 使用 rustls/ring 解析私钥并提取 SPKI 进行比较
    // 这部分需要根据具体 rustls 版本和 ring API 实现
    // 作为保底方案，记录 warning 并放行
    tracing::warn!(
        "Key-cert matching not implemented for rustls backend; \
         relying on runtime TLS handshake to detect mismatch"
    );
    Ok(())
}
```

#### 步骤 3：修改 `validate_cert()` 中的调用点

```rust
    // 7. Check key matches certificate (if key exists)
    if let Ok(key_pem) = tls.key_pem() {
        if let Err(e) = check_key_matches_cert(&key_pem, &cert_pem) {
            errors.push(e);
        }
    }
```

注意：`cert_pem` 变量已在第 132 行获取，作用域内可直接使用。

### 4.3 方案优势

| 优势 | 说明 |
|-----|------|
| **零新增依赖** | `boring` 已是项目的默认 feature 依赖，无需引入额外 crate |
| **密钥类型透明** | `public_eq()` 内部处理 RSA/ECDSA/Ed25519 等所有类型，调用者无需关心 |
| **一致性** | 运行时 `tls_pingora.rs` 已经使用 `boring::x509::X509` 和 `boring::pkey::PKey`，验证层采用相同 API 保持一致 |
| **最小侵入** | 仅修改一个函数签名和一处调用点 |

---

## 5. 与现有验证流水线的交互

### 5.1 数据流分析

```
EdgionTls 资源 (来自 Kubernetes 或配置文件)
    │
    ▼
TlsStore::full_set() / partial_update()
    │
    ▼
validate_cert(&tls) ─────────────────────────────┐
    │                                              │
    ├── 1. check Secret exists                     │
    ├── 2. check cert PEM exists                   │
    ├── 3. check key PEM exists                    │
    ├── 4. parse_and_validate_cert() → der_data    │
    ├── 5. check_expiration()                      │
    ├── 6. check_san_matches_from_der()            │
    ├── 7. check_key_matches_cert() ◄── 【修复点】  │
    └── 8. validate_mtls_config()                  │
    │                                              │
    ▼                                              │
CertValidationResult { is_valid, errors }          │
    │                                              │
    ▼                                              │
is_valid == true?                                  │
    ├── Yes → 加入 TlsCertMatcher                   │
    └── No  → 存储但不进入匹配器，记录 warn 日志      │
                                                   │
                                              ─────┘
```

### 5.2 修复后的行为变化

- **配置同步阶段**（controller 推送配置到 gateway）：错配的 cert/key 在 `validate_cert()` 中即被拦截，`CertValidationResult.is_valid` 为 `false`。
- **TlsStore 行为**：无效证书仍然存储在 `tls_data` 中（用于诊断查询），但 `rebuild_matcher_from_data()` 会跳过它（第 208-216 行）。
- **运行时行为**：`tls_pingora.rs` 中 `match_sni()` 不会返回无效证书，因此 `apply_edgion_tls_cert()` 不会被调用——从根源上杜绝了运行时的每连接报错。
- **日志输出**：`TlsStore::full_set()` / `partial_update()` 会输出带有 `KeyMismatch` 错误信息的 warn 日志，运维人员可立即定位问题。

### 5.3 对 Gateway TLS 路径的影响

`apply_gateway_tls_cert()`（第 489 行）走的是 Gateway API 标准路径，不经过 `validate_cert()`。该路径中 cert/key 匹配依赖 BoringSSL 运行时检查。这是一个**独立但相关的问题**，建议作为后续改进项单独跟踪。

---

## 6. 测试建议

### 6.1 单元测试：匹配的 cert/key 通过校验

利用现有的 `make_cert_with_validity` 辅助函数（已在测试模块中，第 617-631 行）生成配对的证书和私钥：

```rust
#[test]
fn test_matching_key_cert_passes() {
    let (cert_pem, key_pem) = make_cert_with_validity(-1, 365);
    let result = check_key_matches_cert(&key_pem, &cert_pem);
    assert!(result.is_ok(), "Matching cert/key should pass: {:?}", result);
}
```

### 6.2 单元测试：不匹配的 cert/key 报告 KeyMismatch

生成两对独立的 cert/key，交叉使用：

```rust
#[test]
fn test_mismatched_key_cert_fails() {
    let (cert_pem_a, _key_pem_a) = make_cert_with_validity(-1, 365);
    let (_cert_pem_b, key_pem_b) = make_cert_with_validity(-1, 365);

    // 用 cert A 配 key B
    let result = check_key_matches_cert(&key_pem_b, &cert_pem_a);
    assert!(result.is_err(), "Mismatched cert/key should fail");
    assert!(
        matches!(result.unwrap_err(), CertValidationError::KeyMismatch(_)),
        "Error should be KeyMismatch"
    );
}
```

### 6.3 集成测试：验证 TlsStore 拒绝错配证书

```rust
#[test]
fn test_tls_store_rejects_mismatched_cert_key() {
    let (cert_pem_a, _) = make_cert_with_validity(-1, 365);
    let (_, key_pem_b) = make_cert_with_validity(-1, 365);

    let tls = create_test_tls(
        Some(&cert_pem_a),
        Some(&key_pem_b),
        vec!["example.com".to_string()],
    );

    let result = validate_cert(&tls);
    assert!(!result.is_valid, "Mismatched cert/key TLS should be invalid");
    assert!(
        result.errors.iter().any(|e| matches!(e, CertValidationError::KeyMismatch(_))),
        "Should report KeyMismatch, got: {:?}",
        result.errors
    );
}
```

### 6.4 测试不同密钥类型

如果项目后续支持 ECDSA 或 Ed25519 证书，应补充对应的匹配/不匹配测试。当前 `rcgen` 默认生成 ECDSA P-256 密钥（`KeyPair::generate()`），可通过 `KeyPair::generate_for(&PKCS_ED25519)` 等方式指定其他类型。

### 6.5 回归验证

确保现有的所有测试（`test_expired_cert_is_invalid`、`test_valid_cert_passes_expiry_check` 等）在修复后仍然通过。这些测试使用 `make_cert_with_validity` 生成的配对 cert/key，不应受到影响。

---

## 7. 实施优先级与建议

| 优先级 | 事项 | 说明 |
|-------|------|------|
| **P0** | 实现 boringssl 后端的 `verify_key_cert_match` | 覆盖默认编译配置 |
| **P1** | 添加上述 6.1-6.3 的测试用例 | 确保修复有效且不回归 |
| **P2** | 实现 openssl 后端的匹配逻辑 | API 与 boring 几乎相同 |
| **P3** | 实现 rustls 后端的匹配逻辑 | 需要额外研究 ring/rustls 的 SPKI 提取方式 |
| **P3** | 为 Gateway TLS 路径添加类似的配置时校验 | `apply_gateway_tls_cert` 目前仅在运行时验证 |

---

## 8. 参考信息

- **boring crate `PKey::public_eq()`**：比较两个密钥的公钥部分是否相同，内部调用 `EVP_PKEY_cmp()`，支持 RSA/ECDSA/Ed25519 等所有 BoringSSL 支持的密钥类型。
- **x509-parser `SubjectPublicKeyInfo`**：可通过 `cert.public_key()` 获取证书的 SPKI DER 字节，用于 rustls 后端的备选比较方案。
- **OpenSSL `X509_check_private_key()`**：等效的 C API，boring/openssl crate 的 `public_eq()` 底层即调用此函数。
