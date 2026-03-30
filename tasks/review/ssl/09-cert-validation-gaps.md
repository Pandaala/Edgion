# SSL/TLS 代码审查：证书验证缺陷分析

**审查编号**: SSL-09  
**审查日期**: 2026-03-30  
**审查文件**: `src/core/gateway/tls/validation/cert.rs`  
**对比参考**: `src/core/gateway/tls/runtime/backend/cert_extractor.rs`  
**严重程度**: 高（影响生产环境证书链部署与 IP SAN 场景）

---

## 目录

1. [问题一：证书链验证缺失](#1-证书链验证缺失)
2. [问题二：IP SAN 支持缺失](#2-ip-san-支持缺失)
3. [问题三：CN 回退行为不符合现代 TLS 规范](#3-cn-回退行为不符合现代-tls-规范)
4. [影响范围总结](#4-影响范围总结)
5. [修复优先级建议](#5-修复优先级建议)

---

## 1. 证书链验证缺失

### 1.1 问题描述

`parse_and_validate_cert()` 函数（第 186 行）在解析 PEM 数据时，仅读取**第一个 PEM 块**，完全忽略证书链中的中间证书和根证书。

### 1.2 受影响代码

**文件**: `src/core/gateway/tls/validation/cert.rs`，第 186-201 行

```rust
fn parse_and_validate_cert(pem_str: &str) -> Result<(Vec<u8>, i64, i64), String> {
    let pem = Pem::iter_from_buffer(pem_str.as_bytes())
        .next()   // ← 关键问题：只取第一个 PEM 块
        .ok_or_else(|| "No PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {}", e))?;

    let der_data = pem.contents.to_vec();
    let (_, cert) = X509Certificate::from_der(&der_data)
        .map_err(|e| format!("X.509 parse error: {}", e))?;

    let not_before_ts = cert.validity().not_before.timestamp();
    let not_after_ts = cert.validity().not_after.timestamp();

    Ok((der_data, not_before_ts, not_after_ts))
}
```

`Pem::iter_from_buffer()` 返回一个迭代器，可以遍历 PEM 数据中的所有证书块。调用 `.next()` 只消费第一个元素（即叶子证书），后续的中间证书和根证书被完全丢弃。

### 1.3 对比：同项目中的正确实现

在 `src/core/gateway/plugins/http/header_cert_auth/plugin.rs` 第 84-98 行，`parse_ca_bundle_to_der()` 正确地遍历了所有 PEM 块：

```rust
fn parse_ca_bundle_to_der(bundle: &[u8]) -> Vec<Vec<u8>> {
    let mut certs = Vec::new();
    for pem in Pem::iter_from_buffer(bundle).flatten() {
        if pem.label == "CERTIFICATE" && !pem.contents.is_empty() {
            certs.push(pem.contents);
        }
    }
    // ...
}
```

### 1.4 影响分析

**受影响的生产场景**：

| 场景 | 影响 |
|------|------|
| Let's Encrypt 证书（包含中间 CA） | 中间证书未被验证过期时间，可能已过期但验证通过 |
| 企业 PKI 多级证书链 | 中间 CA 吊销或过期不会被检测到 |
| 证书更新时链不完整 | 用户只更新了叶子证书但中间证书已过期，验证仍通过 |
| fullchain.pem 标准部署格式 | 大量用户使用 `fullchain.pem`（Certbot 默认输出），链中证书被忽略 |

**具体风险**：
- 中间 CA 证书过期后，客户端 TLS 握手失败（浏览器报错 `ERR_CERT_DATE_INVALID`），但 Edgion 的验证报告该证书有效，运维无法从 Edgion 侧获得预警
- 证书链顺序错误（例如中间证书放在叶子证书前面）不会被检测到
- 无法验证证书链的签名完整性（叶子证书是否由中间 CA 签发）

### 1.5 根因分析

设计时仅考虑了单证书场景，未考虑 PEM 文件中包含完整证书链的标准部署模式。`.next()` 调用是一个典型的"只处理 happy path"的实现遗漏。

### 1.6 推荐修复

#### 方案 A：解析并验证完整证书链（推荐）

```rust
/// 解析 PEM 中的所有证书，返回 (叶子证书 DER, 所有证书 DER 列表, not_before, not_after)
fn parse_and_validate_cert_chain(
    pem_str: &str,
) -> Result<(Vec<u8>, Vec<Vec<u8>>, i64, i64), String> {
    let certs: Vec<Pem> = Pem::iter_from_buffer(pem_str.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("PEM parse error: {}", e))?;

    if certs.is_empty() {
        return Err("No PEM block found".to_string());
    }

    // 叶子证书（第一个）
    let leaf_der = certs[0].contents.to_vec();
    let (_, leaf_cert) = X509Certificate::from_der(&leaf_der)
        .map_err(|e| format!("Leaf certificate X.509 parse error: {}", e))?;

    let not_before_ts = leaf_cert.validity().not_before.timestamp();
    let not_after_ts = leaf_cert.validity().not_after.timestamp();

    // 验证链中所有中间证书
    let mut all_der = vec![leaf_der.clone()];
    for (i, pem) in certs.iter().enumerate().skip(1) {
        let der = pem.contents.to_vec();
        let (_, intermediate) = X509Certificate::from_der(&der)
            .map_err(|e| format!("Chain certificate [{}] X.509 parse error: {}", i, e))?;

        // 检查中间证书是否过期
        let inter_not_before = intermediate.validity().not_before.timestamp();
        let inter_not_after = intermediate.validity().not_after.timestamp();
        check_expiration(inter_not_before, inter_not_after)
            .map_err(|_| format!(
                "Chain certificate [{}] (subject: {}) has expired or is not yet valid",
                i,
                intermediate.subject()
            ))?;

        all_der.push(der);
    }

    Ok((all_der[0].clone(), all_der, not_before_ts, not_after_ts))
}
```

#### 方案 B：仅验证中间证书有效期（最小改动）

如果暂不实现完整的链签名验证，至少应解析并检查所有中间证书的有效期：

```rust
fn parse_and_validate_cert(pem_str: &str) -> Result<(Vec<u8>, i64, i64), String> {
    let mut pem_iter = Pem::iter_from_buffer(pem_str.as_bytes());

    let leaf_pem = pem_iter
        .next()
        .ok_or_else(|| "No PEM block found".to_string())?
        .map_err(|e| format!("PEM parse error: {}", e))?;

    let der_data = leaf_pem.contents.to_vec();
    let (_, cert) = X509Certificate::from_der(&der_data)
        .map_err(|e| format!("X.509 parse error: {}", e))?;

    let not_before_ts = cert.validity().not_before.timestamp();
    let not_after_ts = cert.validity().not_after.timestamp();

    // 验证链中的中间证书有效期
    for (idx, pem_result) in pem_iter.enumerate() {
        let chain_pem = pem_result
            .map_err(|e| format!("Chain cert [{}] PEM parse error: {}", idx + 1, e))?;
        let (_, chain_cert) = X509Certificate::from_der(&chain_pem.contents)
            .map_err(|e| format!("Chain cert [{}] X.509 parse error: {}", idx + 1, e))?;

        let chain_not_after = chain_cert.validity().not_after.timestamp();
        let chain_not_before = chain_cert.validity().not_before.timestamp();

        if let Err(_) = check_expiration(chain_not_before, chain_not_after) {
            return Err(format!(
                "Intermediate certificate [{}] (subject: {}) is expired or not yet valid",
                idx + 1,
                chain_cert.subject()
            ));
        }
    }

    Ok((der_data, not_before_ts, not_after_ts))
}
```

### 1.7 相关 RFC 参考

- **RFC 5246 § 7.4.2** (TLS 1.2)：服务端发送的 `Certificate` 消息应包含完整的证书链（叶子 → 中间 CA → 根 CA）
- **RFC 8446 § 4.4.2** (TLS 1.3)：同上，明确要求发送者提供完整的证书链
- **RFC 5280 § 6** (X.509 Path Validation)：定义了证书路径验证算法，包括链中每个证书的有效期检查

---

## 2. IP SAN 支持缺失

### 2.1 问题描述

`check_san_matches_from_der()` 函数（第 238-285 行）在提取 SAN 扩展时，仅处理 `GeneralName::DNSName` 类型的条目，完全忽略了 `GeneralName::IPAddress` 类型。当证书包含 IP 地址 SAN（如 `IP:10.0.0.1`）且 host 声明为 IP 地址时，验证将错误地失败并报告 `SanMismatch`。

### 2.2 受影响代码

**文件**: `src/core/gateway/tls/validation/cert.rs`，第 247-257 行

```rust
for ext in cert.extensions() {
    if ext.oid == x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME {
        if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
            for name in &san.general_names {
                if let GeneralName::DNSName(dns) = name {  // ← 只处理 DNS 名称
                    san_list.push(dns.to_string());
                }
                // ← 缺少: GeneralName::IPAddress 处理
            }
        }
        break;
    }
}
```

### 2.3 对比：同项目中的正确实现

`cert_extractor.rs` 第 64-113 行的 `extract_sans()` 函数正确处理了多种 SAN 类型：

```rust
fn extract_sans(cert: &X509Ref) -> Vec<String> {
    let mut sans = Vec::new();
    if let Some(san_ext) = cert.subject_alt_names() {
        for san in san_ext {
            if let Some(dns_name) = san.dnsname() {
                sans.push(dns_name.to_string());
            }
            // IP 地址处理 - IPv4 和 IPv6
            if let Some(ip_bytes) = san.ipaddress() {
                let ip_str = match ip_bytes.len() {
                    4 => format!("{}.{}.{}.{}", ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3]),
                    16 => {
                        let octets: [u8; 16] = ip_bytes.try_into()?;
                        std::net::Ipv6Addr::from(octets).to_string()
                    }
                    _ => continue,
                };
                sans.push(format!("IP:{}", ip_str));
            }
            // ... email 和 URI 处理
        }
    }
    sans
}
```

**关键差距**：同一个项目中，`cert_extractor.rs`（运行时路径）已经正确实现了 IP SAN 提取，而 `cert.rs`（验证路径）遗漏了。这导致运行时能正确识别 IP SAN，但配置验证阶段会误报 SanMismatch 错误。

### 2.4 影响分析

**受影响的生产场景**：

| 场景 | 影响 |
|------|------|
| Kubernetes 内部服务（IP 通信） | 服务间通过 Pod IP 或 ClusterIP 通信，使用 IP SAN 证书无法通过验证 |
| 物联网/边缘设备（无 DNS） | 设备通过 IP 直连网关，使用 IP SAN 证书被拒绝 |
| 开发/测试环境 | 使用自签名 IP SAN 证书的本地开发环境无法正常配置 |
| IPv6 部署 | IPv6 SAN 完全不被识别 |

**具体报错示例**：

```
Certificate:
    Subject Alternative Name:
        IP Address: 10.0.0.1
        IP Address: 192.168.1.100

EdgionTls spec:
    hosts: ["10.0.0.1"]

验证结果: SanMismatch { declared: ["10.0.0.1"], actual: [] }
↑ actual 为空，因为 IP SAN 未被提取
```

### 2.5 根因分析

`check_san_matches_from_der()` 使用 `x509_parser` 库解析 SAN 扩展，该库的 `GeneralName` 枚举包含多种类型（`DNSName`、`IPAddress`、`RFC822Name`、`URI` 等）。当前代码只匹配了 `DNSName` 分支，遗漏了 `IPAddress` 分支。

注意：`x509_parser` 中 `GeneralName::IPAddress` 的值是原始字节（`&[u8]`），IPv4 为 4 字节，IPv6 为 16 字节，需要手动转换为可读的 IP 地址字符串。这与 `cert_extractor.rs` 中使用 BoringSSL/OpenSSL API（返回结构化 IP 数据）的处理方式不同。

### 2.6 推荐修复

修改 `check_san_matches_from_der()` 中的 SAN 提取逻辑：

```rust
fn check_san_matches_from_der(der_data: &[u8], declared_hosts: &[String]) -> Result<(), CertValidationError> {
    let (_, cert) = X509Certificate::from_der(der_data)
        .map_err(|e| CertValidationError::CertParseError(format!("Failed to re-parse cert: {}", e)))?;

    let mut san_dns_list = Vec::new();
    let mut san_ip_list = Vec::new();

    for ext in cert.extensions() {
        if ext.oid == x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME {
            if let ParsedExtension::SubjectAlternativeName(san) = ext.parsed_extension() {
                for name in &san.general_names {
                    match name {
                        GeneralName::DNSName(dns) => {
                            san_dns_list.push(dns.to_string());
                        }
                        GeneralName::IPAddress(ip_bytes) => {
                            match ip_bytes.len() {
                                4 => {
                                    let ip = std::net::Ipv4Addr::new(
                                        ip_bytes[0], ip_bytes[1], ip_bytes[2], ip_bytes[3],
                                    );
                                    san_ip_list.push(ip.to_string());
                                }
                                16 => {
                                    if let Ok(octets) = <[u8; 16]>::try_from(*ip_bytes) {
                                        let ip = std::net::Ipv6Addr::from(octets);
                                        san_ip_list.push(ip.to_string());
                                    }
                                }
                                _ => {} // 忽略无效长度
                            }
                        }
                        _ => {} // 其他类型暂不处理
                    }
                }
            }
            break;
        }
    }

    // CN 回退（仅当 DNS SAN 和 IP SAN 都为空时）
    if san_dns_list.is_empty() && san_ip_list.is_empty() {
        if let Some(cn) = cert.subject().iter_common_name().next() {
            if let Ok(cn_str) = cn.as_str() {
                san_dns_list.push(cn_str.to_string());
            }
        }
    }

    let mut missing_hosts = Vec::new();
    for declared_host in declared_hosts {
        let is_ip = declared_host.parse::<std::net::IpAddr>().is_ok();
        let covered = if is_ip {
            san_ip_list.iter().any(|ip| ip == declared_host)
        } else {
            is_host_covered(declared_host, &san_dns_list)
        };
        if !covered {
            missing_hosts.push(declared_host.clone());
        }
    }

    if !missing_hosts.is_empty() {
        let mut all_sans: Vec<String> = san_dns_list;
        all_sans.extend(san_ip_list.into_iter().map(|ip| format!("IP:{}", ip)));
        return Err(CertValidationError::SanMismatch {
            declared: declared_hosts.to_vec(),
            actual: all_sans,
        });
    }

    Ok(())
}
```

**要点说明**：

1. IP 地址匹配**不应使用通配符**——RFC 6125 § 1.7.2 明确规定通配符匹配仅适用于 DNS 域名，不适用于 IP 地址
2. IP 匹配应使用规范化比较（如 `std::net::IpAddr` 解析后比较），以正确处理 IPv6 地址的多种表示形式（如 `::1` vs `0:0:0:0:0:0:0:1`）
3. 将 DNS SAN 和 IP SAN 分开存储，host 声明为 IP 地址时只在 IP SAN 列表中查找

### 2.7 同步修改 `is_host_covered()`

当前 `is_host_covered()` 对 IP 地址不需要修改，因为 IP 匹配逻辑在上层函数中已独立处理。但建议添加一个显式注释说明该函数仅用于 DNS 名称匹配。

### 2.8 相关 RFC 参考

- **RFC 5280 § 4.2.1.6**：定义了 SubjectAlternativeName 扩展，包括 `iPAddress` 类型（OID 2.5.29.17）
- **RFC 6125 § 1.7.2**：明确 IP 地址不适用通配符匹配规则
- **RFC 6125 § 6.4.4**：IP 地址的匹配必须是完全匹配（字节级比较）

---

## 3. CN 回退行为不符合现代 TLS 规范

### 3.1 问题描述

当 SAN 扩展为空时，代码回退到使用 Subject 中的 Common Name (CN) 进行匹配（第 261-267 行），但存在以下问题：

1. **不符合现代标准**：RFC 6125 (2011) 已明确废弃 CN 匹配，建议实现应仅使用 SAN
2. **无日志告警**：回退到 CN 时没有任何日志输出，运维无法感知这一行为
3. **回退条件过于宽泛**：仅判断 `san_list.is_empty()`，但实际上应区分"证书无 SAN 扩展"和"证书有 SAN 扩展但内容为空"两种情况

### 3.2 受影响代码

**文件**: `src/core/gateway/tls/validation/cert.rs`，第 260-267 行

```rust
// If no SAN, try to get CN from subject
if san_list.is_empty() {
    if let Some(cn) = cert.subject().iter_common_name().next() {
        if let Ok(cn_str) = cn.as_str() {
            san_list.push(cn_str.to_string());
        }
    }
}
```

### 3.3 影响分析

| 场景 | 影响 |
|------|------|
| 旧版证书（无 SAN 扩展） | 静默回退到 CN，通过验证但客户端（如 Chrome）已不接受此类证书 |
| 证书有 SAN 但仅包含 IP 类型 | 因当前代码未提取 IP SAN，`san_list` 为空，错误地回退到 CN |
| 安全审计 | 审计人员无法从日志中发现 CN 回退行为 |

**特别注意**：问题 2（IP SAN 缺失）和问题 3 存在联动效应。当证书仅包含 IP SAN 时：
1. 当前代码不提取 IP SAN → `san_list` 为空
2. 触发 CN 回退 → 尝试用 CN 匹配 IP 地址
3. CN 通常是域名而非 IP → 匹配失败 → 报告 SanMismatch

即使 CN 恰好是 IP 地址，也不应通过 CN 匹配来覆盖 IP SAN 的功能。

### 3.4 推荐修复

```rust
// 判断是否存在 SAN 扩展（而非 san_list 是否为空）
let has_san_extension = cert.extensions().iter().any(|ext| {
    ext.oid == x509_parser::oid_registry::OID_X509_EXT_SUBJECT_ALT_NAME
});

if !has_san_extension {
    // 证书不含 SAN 扩展，回退到 CN（兼容旧证书）
    tracing::warn!(
        "Certificate for hosts {:?} has no SAN extension, falling back to CN matching. \
         This is deprecated per RFC 6125 and may cause issues with modern TLS clients.",
        declared_hosts
    );
    if let Some(cn) = cert.subject().iter_common_name().next() {
        if let Ok(cn_str) = cn.as_str() {
            san_dns_list.push(cn_str.to_string());
        }
    }
}
```

### 3.5 相关 RFC 参考

- **RFC 6125 § 6.4.4**："实现不应（SHOULD NOT）在 subjectAltName 扩展存在时尝试匹配 CN-ID"
- **RFC 2818 § 3.1** (HTTPS)：最初定义了 CN 作为回退，但已被 RFC 6125 取代
- **CA/Browser Forum Baseline Requirements § 7.1.4.2**：自 2012 年起，公开信任的 CA 必须在 SAN 中包含所有域名

---

## 4. 影响范围总结

### 4.1 组件关系图

```
EdgionTls 资源变更
       │
       ▼
validate_cert()          ← 入口点
       │
       ├─► parse_and_validate_cert()    ← 问题1: 只解析第一个 PEM
       │          │
       │          ▼
       ├─► check_expiration()           ← 仅对叶子证书检查
       │
       ├─► check_san_matches_from_der() ← 问题2: 忽略 IP SAN
       │          │                     ← 问题3: CN 回退无告警
       │          ▼
       │   is_host_covered()            ← 仅 DNS 名称匹配（本身无问题）
       │
       └─► check_key_valid()            ← 不受影响
```

### 4.2 风险矩阵

| 问题 | 严重程度 | 发生概率 | 检测难度 | 综合风险 |
|------|----------|----------|----------|----------|
| 证书链验证缺失 | 高 | 高（绝大部分生产证书含链） | 中（链过期后客户端报错才发现） | **高** |
| IP SAN 支持缺失 | 中 | 中（内部服务 IP 通信） | 低（配置时即报错） | **中** |
| CN 回退无告警 | 低 | 低（现代证书都有 SAN） | 高（静默行为） | **低** |

---

## 5. 修复优先级建议

### P0：IP SAN 支持（建议立即修复）

- **理由**：这是一个功能性缺陷，用户使用 IP SAN 证书时会直接遇到 `SanMismatch` 错误，无法完成配置。同项目的 `cert_extractor.rs` 已有正确实现可参考。
- **工作量**：小（约 30 行代码改动 + 测试）
- **风险**：低（新增功能，不影响现有 DNS SAN 逻辑）

### P1：证书链有效期验证（建议近期修复）

- **理由**：大部分生产部署使用包含中间 CA 的证书链。虽然 Pingora 运行时会做完整的 TLS 握手验证，但 Edgion 的预验证阶段应尽早发现问题并通过 Status 反馈给用户。
- **工作量**：中（约 50 行代码改动 + 测试）
- **风险**：低（只增加额外检查，不影响叶子证书验证逻辑）

### P2：CN 回退告警（建议后续迭代）

- **理由**：现代 CA 签发的证书都包含 SAN 扩展，CN 回退主要影响极端边缘情况。添加日志告警即可。
- **工作量**：极小（约 5 行代码）
- **风险**：无

---

## 6. 建议的测试用例

### 6.1 证书链相关测试

```rust
#[test]
fn test_cert_chain_all_certs_validated() {
    // 构造一个包含 leaf + intermediate 的 PEM 文件
    // intermediate 已过期
    // 验证应返回错误（当前实现会通过 → 证明问题存在）
}

#[test]
fn test_cert_chain_intermediate_not_yet_valid() {
    // intermediate 的 not_before 在未来
    // 应返回错误
}

#[test]
fn test_cert_chain_valid() {
    // leaf + intermediate 都在有效期内
    // 应通过验证
}

#[test]
fn test_single_cert_still_works() {
    // 保持向后兼容：单个证书 PEM 仍然正常工作
}
```

### 6.2 IP SAN 相关测试

```rust
#[test]
fn test_ip_san_ipv4_match() {
    // 证书 SAN 包含 IP:10.0.0.1
    // declared_hosts: ["10.0.0.1"]
    // 应通过验证
}

#[test]
fn test_ip_san_ipv6_match() {
    // 证书 SAN 包含 IP:fd00::1
    // declared_hosts: ["fd00::1"]
    // 应通过验证
}

#[test]
fn test_ip_san_no_wildcard_match() {
    // 验证 IP 地址不支持通配符匹配
    // 证书 SAN: IP:10.0.0.1
    // declared_hosts: ["10.0.0.*"]
    // 不应通过验证
}

#[test]
fn test_mixed_dns_and_ip_san() {
    // 证书 SAN 同时包含 DNS:example.com 和 IP:10.0.0.1
    // declared_hosts: ["example.com", "10.0.0.1"]
    // 两者都应通过验证
}

#[test]
fn test_ip_host_not_matched_by_dns_san() {
    // 证书 SAN 仅包含 DNS:example.com
    // declared_hosts: ["10.0.0.1"]
    // 应返回 SanMismatch
}
```

### 6.3 CN 回退相关测试

```rust
#[test]
fn test_cn_fallback_no_san_extension() {
    // 证书无 SAN 扩展，CN = "example.com"
    // declared_hosts: ["example.com"]
    // 应通过验证（兼容旧证书）
    // 日志应包含 warn 级别告警
}

#[test]
fn test_cn_not_used_when_san_exists() {
    // 证书有 SAN 扩展（仅 DNS:other.com），CN = "example.com"
    // declared_hosts: ["example.com"]
    // 不应通过验证（SAN 存在时不应回退 CN）
}
```

### 6.4 构造测试证书的辅助方法

项目已使用 `rcgen` crate 生成测试证书（见 `make_cert_with_validity()`）。建议扩展该辅助方法以支持：

```rust
/// 生成带有指定 SAN 的自签名证书
fn make_cert_with_sans(
    dns_names: Vec<&str>,
    ip_addrs: Vec<std::net::IpAddr>,
) -> (String, String) {
    use rcgen::{CertificateParams, KeyPair, SanType};

    let mut san_types: Vec<SanType> = dns_names
        .into_iter()
        .map(|name| SanType::DnsName(name.try_into().unwrap()))
        .collect();
    san_types.extend(ip_addrs.into_iter().map(SanType::IpAddress));

    let key_pair = KeyPair::generate().expect("key generation failed");
    let mut params = CertificateParams::new(vec![]).expect("params failed");
    params.subject_alt_names = san_types;
    let cert = params.self_signed(&key_pair).expect("self-sign failed");
    (cert.pem(), key_pair.serialize_pem())
}

/// 生成包含 leaf + intermediate 的证书链 PEM
fn make_cert_chain(
    leaf_not_before_days: i64,
    leaf_not_after_days: i64,
    intermediate_not_before_days: i64,
    intermediate_not_after_days: i64,
) -> (String, String) {
    // 使用 rcgen 生成 CA 证书和叶子证书
    // 返回 (chain_pem: leaf + intermediate 拼接, key_pem)
    todo!("实现证书链生成")
}
```

---

## 附录：相关文件清单

| 文件 | 角色 | 相关问题 |
|------|------|----------|
| `src/core/gateway/tls/validation/cert.rs` | 证书预验证（配置阶段） | 问题 1, 2, 3 |
| `src/core/gateway/tls/runtime/backend/cert_extractor.rs` | 运行时客户端证书信息提取 | 参考实现（IP SAN） |
| `src/core/gateway/plugins/http/header_cert_auth/plugin.rs` | CA Bundle 解析 | 参考实现（完整 PEM 遍历） |
| `src/core/gateway/tls/boringssl/mtls_verify_callback.rs` | 运行时证书链验证回调 | 运行时已有链验证 |
| `src/core/gateway/tls/runtime/gateway/tls_pingora.rs` | Pingora TLS 配置 | CA 证书加载逻辑 |
