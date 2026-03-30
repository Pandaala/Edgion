# TLS 配置中的静默失败与低效问题

**审查编号**: SSL-08
**审查日期**: 2026-03-30
**严重等级**: 高（静默失败）/ 中（竞态条件）/ 低（性能浪费）
**影响范围**: TLS 握手路径、后端 TLS 存储、证书验证流程

---

## 概述

本审查发现 Edgion TLS 子系统中存在四类相互关联的问题：cipher 配置静默吞掉错误、后端 TLS 存储的非原子操作引发竞态条件、证书验证中的冗余解析，以及 `tls_id` 生成策略的碰撞风险。这些问题组合起来会导致：配置错误无法被运维人员发现、并发更新丢失数据、每次连接浪费 CPU 做重复解析、以及在高并发场景下可能出现日志追踪 ID 碰撞。

---

## 问题 1：Cipher 配置静默失败

### 影响文件

`src/core/gateway/tls/runtime/gateway/tls_pingora.rs`，第 804–838 行（`configure_ciphers` 方法）

### 问题描述

`configure_ciphers` 方法在两个关键失败点上返回 `Ok(())`，完全隐藏了错误：

```804:838:src/core/gateway/tls/runtime/gateway/tls_pingora.rs
    fn configure_ciphers(&self, ssl: &mut SslRef, ciphers: &[String]) -> Result<(), Box<PingoraError>> {
        if ciphers.is_empty() {
            return Ok(());
        }

        let cipher_list = ciphers.join(":");

        #[cfg(feature = "boringssl")]
        {
            use std::ffi::CString;

            let cipher_cstr = match CString::new(cipher_list.as_str()) {
                Ok(c) => c,
                Err(_) => return Ok(()),
            };

            // SAFETY: SslRef -> SSL* conversion for FFI call
            // ssl is valid during this function, cipher_cstr lifetime extends past FFI call
            unsafe {
                let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
                let ret = boring_sys::SSL_set_strict_cipher_list(ssl_ptr, cipher_cstr.as_ptr());

                if ret != 1 {
                    return Ok(());
                }
            }
        }

        #[cfg(not(feature = "boringssl"))]
        {
            let _ = ssl;
        }

        Ok(())
    }
```

**静默失败点 A**（第 816–818 行）：`CString::new()` 失败时（cipher 字符串中包含 `\0` 字节），直接返回 `Ok(())`，不报告任何错误。

**静默失败点 B**（第 826–828 行）：`SSL_set_strict_cipher_list()` 返回 0（表示 cipher 字符串无效或所有 cipher 都不被支持）时，同样返回 `Ok(())`。

### 调用端的处理

调用者位于 `apply_edgion_tls_cert` 第 477–485 行：

```477:485:src/core/gateway/tls/runtime/gateway/tls_pingora.rs
        if let Some(ref ciphers) = edgion_tls.spec.ciphers {
            if let Err(e) = self.configure_ciphers(ssl, ciphers) {
                let _ = ssl_ctx
                    .meta
                    .log
                    .get_or_insert_with(LogBuffer::new)
                    .push(&format!("[err] cipher: {e}"));
            }
        }
```

由于 `configure_ciphers` 永远不会返回 `Err`，调用端的错误日志分支**永远不会被执行**。即使调用端设计了错误处理逻辑，也形同虚设。

### 影响分析

| 维度 | 影响 |
|------|------|
| **安全性** | 运维人员配置了自定义 cipher 列表（例如仅允许 TLS_AES_256_GCM_SHA384），但如果配置字符串有误，连接会静默回退到 BoringSSL 默认 cipher 集合，可能包含运维人员试图禁止的弱 cipher |
| **可观测性** | ssl.log 中不会出现任何 cipher 配置失败的记录，运维人员无法发现配置未生效 |
| **合规性** | 在需要严格 cipher 控制的合规场景（PCI-DSS、HIPAA）中，cipher 策略可能在运维不知情的情况下未被执行 |

### 根因

函数签名返回 `Result<(), Box<PingoraError>>`，但实现中从未构造过 `Err`。这是典型的"签名承诺了错误处理但实现没有兑现"的模式。

### 修复建议

将失败路径改为返回实际的错误：

```rust
fn configure_ciphers(
    &self,
    ssl: &mut SslRef,
    ciphers: &[String],
) -> Result<(), Box<PingoraError>> {
    if ciphers.is_empty() {
        return Ok(());
    }

    let cipher_list = ciphers.join(":");

    #[cfg(feature = "boringssl")]
    {
        use std::ffi::CString;

        let cipher_cstr = CString::new(cipher_list.as_str()).map_err(|e| {
            PingoraError::explain(
                ErrorType::InternalError,
                format!("cipher string contains null byte: {e}"),
            )
        })?;

        unsafe {
            let ssl_ptr = ssl as *mut SslRef as *mut boring_sys::SSL;
            let ret = boring_sys::SSL_set_strict_cipher_list(
                ssl_ptr,
                cipher_cstr.as_ptr(),
            );

            if ret != 1 {
                return Err(PingoraError::explain(
                    ErrorType::InternalError,
                    format!(
                        "SSL_set_strict_cipher_list rejected ciphers: {}",
                        cipher_list
                    ),
                ));
            }
        }
    }

    Ok(())
}
```

同时建议调用端评估是否需要在 cipher 配置失败时中断连接（返回 `return`），还是仅记录日志继续使用默认 cipher。当前行为是继续（不 return），这可以保留为设计决策，但**前提是错误确实被记录了**。

### 测试建议

- 单元测试：传入包含 null 字节的 cipher 字符串（如 `"AES128\0GCM"`），验证返回 `Err`
- 单元测试：传入无效 cipher 名称（如 `"INVALID_CIPHER_XYZ"`），验证返回 `Err`
- 集成测试：配置一个无效 cipher 列表，验证 ssl.log 中出现对应的错误条目
- 集成测试：确认 cipher 配置失败后连接行为符合预期（回退默认还是拒绝连接）

---

## 问题 2：GatewayBackendTlsStore 非原子操作（TOCTOU 竞态）

### 影响文件

`src/core/gateway/backends/gateway_backend_tls_store.rs`，第 36–51 行

### 问题描述

`GatewayBackendTlsStore` 使用 `ArcSwap<HashMap<...>>` 实现无锁读取，但写操作（`set` 和 `remove`）采用了 load → clone → modify → store 的模式，存在 TOCTOU（Time-of-Check-to-Time-of-Use）竞态条件。

**`set()` 方法（第 36–41 行）：**

```36:41:src/core/gateway/backends/gateway_backend_tls_store.rs
    pub fn set(&self, gateway_key: String, secret: Arc<Secret>) {
        let current = self.store.load();           // T1: 读取当前 map
        let mut new_map = (**current).clone();     // T2: 克隆整个 map
        new_map.insert(gateway_key, secret);       // T3: 插入新条目
        self.store.store(Arc::new(new_map));       // T4: 存回 store
    }
```

**竞态场景**：两个线程同时调用 `set()`：

```
Thread A: load() → clone → insert("gw-1", secret_a) → store()
Thread B: load() → clone → insert("gw-2", secret_b) → store()
```

如果 Thread A 和 Thread B 在各自的 `store()` 之前都完成了 `load()`，它们各自克隆的是同一份旧 map。最终 `store()` 后写入的那个线程会覆盖先写入的，导致一个 gateway 的 TLS 配置丢失。

**`remove()` 方法（第 44–50 行）：**

```44:50:src/core/gateway/backends/gateway_backend_tls_store.rs
    pub fn remove(&self, gateway_key: &str) {
        let current = self.store.load();
        if current.contains_key(gateway_key) {     // 检查 key 存在
            let mut new_map = (**current).clone();  // 两行之后才修改
            new_map.remove(gateway_key);
            self.store.store(Arc::new(new_map));
        }
    }
```

同样的 TOCTOU 问题：`contains_key` 检查和后续的 `remove` + `store` 之间，其他线程可能已经修改了 map。

### 影响分析

| 维度 | 影响 |
|------|------|
| **数据一致性** | 并发更新会导致 last-writer-wins，丢失中间写入的 TLS 客户端证书配置 |
| **后端 mTLS** | 如果后端 TLS 客户端证书丢失，到上游服务的 mTLS 连接会失败（认证错误），但错误原因可能被误判为证书问题而非竞态条件 |
| **复现难度** | 在低频配置更新场景下不容易复现，但在批量更新（如 GitOps 推送多个 Gateway 资源）或控制器 reconcile 风暴期间可能触发 |

### 根因

`ArcSwap` 的 `load()` + `store()` 组合不是原子操作。`ArcSwap` 本身提供了 `rcu()`（read-copy-update）方法来解决这个问题，但代码没有使用。

### 修复建议

使用 `ArcSwap::rcu()` 将 read-modify-write 组合为一个原子操作：

```rust
pub fn set(&self, gateway_key: String, secret: Arc<Secret>) {
    self.store.rcu(|current| {
        let mut new_map = (**current).clone();
        new_map.insert(gateway_key.clone(), secret.clone());
        new_map
    });
}

pub fn remove(&self, gateway_key: &str) {
    self.store.rcu(|current| {
        let mut new_map = (**current).clone();
        new_map.remove(gateway_key);
        new_map
    });
}
```

`rcu()` 内部会自动检测冲突并重试：如果在闭包执行期间底层值被其他线程修改，`rcu` 会用新值重新调用闭包。这保证了最终结果包含所有并发写入。

> **注意**：`remove()` 中的 `contains_key` 前置检查在 `rcu` 模式下可以省略——直接调用 `remove()` 在 key 不存在时是无开销的 no-op（不会克隆 map），但如果想避免无意义的 `rcu` 重试循环开销，可以保留为一个快速路径优化：

```rust
pub fn remove(&self, gateway_key: &str) {
    if !self.store.load().contains_key(gateway_key) {
        return;
    }
    self.store.rcu(|current| {
        let mut new_map = (**current).clone();
        new_map.remove(gateway_key);
        new_map
    });
}
```

### 测试建议

- 并发测试：启动 N 个线程，每个线程 `set` 不同的 key，完成后验证所有 key 都存在
- 并发测试：一个线程不断 `set`，另一个线程不断 `remove`，验证最终状态一致
- 压力测试：在 reconcile 模拟场景下，批量更新 100 个 Gateway 的 backend TLS 配置，验证无数据丢失
- 使用 `loom` 或 `shuttle` 做并发模型检测（如果项目已经引入这些工具）

---

## 问题 3：证书双重解析（性能浪费）

### 影响文件

`src/core/gateway/tls/validation/cert.rs`，第 186–201 行和第 238–241 行

### 问题描述

`validate_cert` 流程中，同一证书被解析了两次：

**第一次解析**（`parse_and_validate_cert`，第 186–201 行）：

```186:201:src/core/gateway/tls/validation/cert.rs
fn parse_and_validate_cert(pem_str: &str) -> Result<(Vec<u8>, i64, i64), String> {
    // Parse PEM
    let pem = Pem::iter_from_buffer(pem_str.as_bytes())
        .next()
        .ok_or_else(|| "No PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {}", e))?;

    // Parse X.509
    let der_data = pem.contents.to_vec();
    let (_, cert) = X509Certificate::from_der(&der_data).map_err(|e| format!("X.509 parse error: {}", e))?;

    // Extract validity as Unix timestamps (avoids string roundtrip)
    let not_before_ts = cert.validity().not_before.timestamp();
    let not_after_ts = cert.validity().not_after.timestamp();

    Ok((der_data, not_before_ts, not_after_ts))
}
```

这里将 PEM 解码为 DER，然后将 DER 解析为 `X509Certificate` 提取有效期信息。返回值中只保留了 `der_data`（原始字节），而 `X509Certificate` 被丢弃。

**第二次解析**（`check_san_matches_from_der`，第 238–241 行）：

```238:241:src/core/gateway/tls/validation/cert.rs
fn check_san_matches_from_der(der_data: &[u8], declared_hosts: &[String]) -> Result<(), CertValidationError> {
    // Parse certificate again for SAN check
    let (_, cert) = X509Certificate::from_der(der_data)
        .map_err(|e| CertValidationError::CertParseError(format!("Failed to re-parse cert: {}", e)))?;
```

代码注释明确写着 "Parse certificate again for SAN check"——作者已经意识到这是重复解析，但没有重构。

### 影响分析

| 维度 | 影响 |
|------|------|
| **性能** | 每次证书验证多一次 ASN.1 DER 解析，X.509 解析涉及 ASN.1 结构遍历、OID 匹配等操作 |
| **热路径程度** | `validate_cert` 在控制器 reconcile 时调用（配置变更时），不在每次 TLS 握手的热路径上，因此性能影响相对有限 |
| **代码质量** | 违反 DRY 原则；如果将来 `X509Certificate::from_der` 行为变更（如更严格的解析），两处需要同步更新 |
| **潜在 bug** | 第二次解析理论上不应失败（因为第一次已经成功），但错误路径仍被处理，增加了不必要的复杂度 |

### 根因

`parse_and_validate_cert` 的返回值设计为 `(Vec<u8>, i64, i64)`，只返回原始 DER 数据和时间戳，没有返回已解析的 `X509Certificate`。这是因为 `x509_parser` 的 `X509Certificate` 持有对输入 `&[u8]` 的引用（zero-copy 设计），不能独立于输入数据存在，直接返回会遇到生命周期问题。

### 修复建议

**方案 A：合并验证逻辑到单个函数**

将过期检查和 SAN 检查合并到 `parse_and_validate_cert` 中，一次解析完成所有验证：

```rust
struct ParsedCertInfo {
    not_before_ts: i64,
    not_after_ts: i64,
    san_list: Vec<String>,
}

fn parse_and_extract_cert_info(pem_str: &str) -> Result<ParsedCertInfo, String> {
    let pem = Pem::iter_from_buffer(pem_str.as_bytes())
        .next()
        .ok_or_else(|| "No PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {}", e))?;

    let der_data = pem.contents.to_vec();
    let (_, cert) = X509Certificate::from_der(&der_data)
        .map_err(|e| format!("X.509 parse error: {}", e))?;

    let not_before_ts = cert.validity().not_before.timestamp();
    let not_after_ts = cert.validity().not_after.timestamp();

    let mut san_list = Vec::new();
    for ext in cert.extensions() {
        if ext.oid == x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME {
            if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
                for name in &san.general_names {
                    if let GeneralName::DNSName(dns) = name {
                        san_list.push(dns.to_string());
                    }
                }
            }
            break;
        }
    }
    if san_list.is_empty() {
        if let Some(cn) = cert.subject().iter_common_name().next() {
            if let Ok(cn_str) = cn.as_str() {
                san_list.push(cn_str.to_string());
            }
        }
    }

    Ok(ParsedCertInfo {
        not_before_ts,
        not_after_ts,
        san_list,
    })
}
```

然后在 `validate_cert` 中：

```rust
let cert_info = match parse_and_extract_cert_info(&cert_pem) {
    Ok(info) => info,
    Err(e) => {
        errors.push(CertValidationError::CertParseError(e));
        return CertValidationResult::invalid(errors);
    }
};

if let Err(e) = check_expiration(cert_info.not_before_ts, cert_info.not_after_ts) {
    errors.push(e);
}

if let Err(e) = check_san_matches(&cert_info.san_list, &tls.spec.hosts) {
    errors.push(e);
}
```

**方案 B：如果要保留函数分离**

可以让 `parse_and_validate_cert` 返回已提取的 SAN 列表，避免需要保存 DER 数据：

```rust
fn parse_and_validate_cert(pem_str: &str) -> Result<(i64, i64, Vec<String>), String> {
    // ... 解析后同时返回时间戳和 SAN 列表
}
```

### 测试建议

- 重构后的回归测试：确保所有现有的 `cert.rs` 单元测试继续通过
- 性能基准（可选）：使用 `criterion` 对比重构前后 `validate_cert` 的耗时，量化改善
- 边界测试：验证没有 SAN 扩展的证书仍然正确回退到 CN 匹配

---

## 问题 4：tls_id 唯一性不足（碰撞风险）

### 影响文件

`src/core/gateway/tls/runtime/gateway/tls_pingora.rs`，第 259–260 行

### 问题描述

```258:260:src/core/gateway/tls/runtime/gateway/tls_pingora.rs
        let rand: u32 = random();
        ssl_ctx.meta.tls_id = Some(format!("{:08x}-{:08x}", ssl_ctx.meta.ts as u32, rand));
```

`tls_id` 由两部分组成：

1. **`ssl_ctx.meta.ts as u32`**：毫秒时间戳截断为 32 位。`ts` 是 `i64` 类型的毫秒级 Unix 时间戳（第 43–46 行），`as u32` 只保留低 32 位。32 位毫秒时间戳的周期约为 2^32 ms ≈ 49.7 天，意味着每 ~50 天时间戳部分会循环一次。

2. **`rand::random::<u32>()`**：32 位随机数，提供约 4.29 × 10^9 种可能值。

总有效熵为 64 位（32 位时间 + 32 位随机），但实际上在同一毫秒内的有效熵只有 32 位。

### 碰撞概率分析（生日悖论）

在同一毫秒内的并发连接（时间戳部分相同，碰撞完全取决于 32 位随机部分）：

| 同一毫秒内并发连接数 | 碰撞概率 |
|---------------------|----------|
| 1,000 | ~0.012% |
| 10,000 | ~1.2% |
| 77,000 | ~50% |

对于高性能 API 网关，同一毫秒内处理数千连接是合理场景。如果 `tls_id` 被用作日志追踪的关键 ID，碰撞会导致多个连接的日志混在一起，给故障排查带来困难。

此外，50 天后时间戳循环叠加 32 位随机部分，使得跨时间窗口的碰撞也有可能（虽然概率很低）。

### 影响分析

| 维度 | 影响 |
|------|------|
| **可观测性** | `tls_id` 碰撞导致 ssl.log 中多个连接共享同一 ID，日志追踪混乱 |
| **调试困难** | 故障排查时通过 `tls_id` 过滤日志可能返回多个不相关连接的条目 |
| **关联性** | 如果 `tls_id` 被下游系统（metrics、tracing）引用，碰撞会破坏端到端追踪链 |

### 根因

设计时可能认为 32 位随机数足够，没有考虑高并发场景下的生日悖论效应。`ts as u32` 的截断也不必要地降低了时间戳部分的有效位数。

### 修复建议

**方案 A（推荐）：使用 64 位随机数 + 完整时间戳**

```rust
let rand: u64 = random();
ssl_ctx.meta.tls_id = Some(format!("{:016x}-{:016x}", ssl_ctx.meta.ts, rand));
```

这将有效熵提升到 128 位（64 位时间戳 + 64 位随机），碰撞概率降到天文级别。ID 格式从 17 字符变为 33 字符，对日志存储影响微乎其微。

**方案 B：使用 UUID v7**

如果项目中已经引入或计划引入 `uuid` crate：

```rust
ssl_ctx.meta.tls_id = Some(uuid::Uuid::now_v7().to_string());
```

UUID v7 天然包含毫秒时间戳和随机部分，且按时间排序，非常适合日志追踪场景。缺点是格式为 36 字符（含连字符）。

**方案 C：最小改动**

如果希望保持现有格式长度，至少移除 `as u32` 截断：

```rust
let rand: u32 = random();
ssl_ctx.meta.tls_id = Some(format!("{:016x}-{:08x}", ssl_ctx.meta.ts, rand));
```

这保留了完整的 64 位时间戳，跨时间窗口碰撞问题消除；同一毫秒内仍然依赖 32 位随机数，但已经大幅改善。

### 测试建议

- 单元测试：生成 100,000 个 tls_id，验证无碰撞
- 属性测试（proptest）：验证格式符合预期（十六进制、正确长度）
- 如果采用方案 A 或 B，验证 `tls_id` 字段在序列化/反序列化（JSON 等）中不会被截断

---

## 综合优先级建议

| 优先级 | 问题 | 理由 |
|--------|------|------|
| **P1（高）** | 问题 1：Cipher 静默失败 | 安全相关，可能导致密码套件策略不生效且无法发现 |
| **P2（中）** | 问题 2：TlsStore 竞态条件 | 数据丢失虽罕见但在批量更新场景下可能触发，影响后端 mTLS |
| **P2（中）** | 问题 4：tls_id 碰撞风险 | 影响可观测性，在高并发场景下有实际碰撞概率 |
| **P3（低）** | 问题 3：双重解析 | 性能浪费但不在 TLS 握手热路径上，主要是代码质量问题 |

### 建议的修复顺序

1. 先修复问题 1（cipher 静默失败），改动最小、收益最大
2. 修复问题 2（TlsStore 竞态），只需将 `load/store` 替换为 `rcu`
3. 修复问题 4（tls_id），一行代码改动
4. 最后重构问题 3（双重解析），涉及函数签名变更和调用端调整，需要更充分的回归测试

---

## 附录：相关代码引用

| 组件 | 文件路径 | 关键行号 |
|------|----------|----------|
| configure_ciphers | `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | 804–838 |
| cipher 调用端 | `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | 477–485 |
| tls_id 生成 | `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | 259–260 |
| GatewayBackendTlsStore::set | `src/core/gateway/backends/gateway_backend_tls_store.rs` | 36–41 |
| GatewayBackendTlsStore::remove | `src/core/gateway/backends/gateway_backend_tls_store.rs` | 44–50 |
| parse_and_validate_cert | `src/core/gateway/tls/validation/cert.rs` | 186–201 |
| check_san_matches_from_der | `src/core/gateway/tls/validation/cert.rs` | 238–241 |
