# SSL/TLS Review #02: 重复的通配符匹配逻辑与不一致行为

- **严重级别**: 高（安全相关）
- **影响范围**: TLS 证书验证、mTLS 客户端认证、后端 SAN 验证、TLS 路由匹配
- **类型**: 代码重复 / 行为不一致 / 安全隐患

---

## 1. 问题描述

通配符主机名/模式匹配逻辑在代码库中被**独立实现了至少 4 次**，且各实现之间存在**微妙但关键的行为差异**。这违反了 DRY（Don't Repeat Yourself）原则，更重要的是，不一致的行为可能导致安全漏洞或难以排查的运行时错误。

### 1.1 四处实现总览

| # | 文件 | 函数 | 行号 | 用途 |
|---|------|------|------|------|
| 1 | `src/core/gateway/tls/validation/cert.rs` | `is_host_covered()` | L288-L317 | 验证证书 SAN 是否覆盖声明的主机名 |
| 2 | `src/core/gateway/tls/validation/mtls.rs` | `matches_pattern()` | L74-L111 | mTLS 客户端证书 SAN/CN 白名单匹配 |
| 3 | `src/core/gateway/tls/validation/backend_san.rs` | `matches_hostname()` | L88-L114 | 后端 TLS 连接的服务端证书 SAN 验证 |
| 4 | `src/types/resources/edgion_tls.rs` | `wildcard_match()` | L383-L467 | EdgionTls 资源的主机名路由匹配 |

### 1.2 行为差异对比表

| 特性 | cert.rs `is_host_covered` | mtls.rs `matches_pattern` | backend_san.rs `matches_hostname` | edgion_tls.rs `wildcard_match` |
|------|:---:|:---:|:---:|:---:|
| **大小写敏感** | 不敏感 (`.to_lowercase()`) | **敏感** | **敏感** | 不敏感 (调用方转小写) |
| **多级通配符** `*.*.example.com` | 不支持 | 不支持 | 不支持 | **支持** |
| **单级通配符** `*.example.com` | 支持 | 支持 | 支持 | 支持 |
| **空前缀保护** (`*.` 无后缀) | 无检查 | `len < 3` 检查 | `len < 3` 检查 | 隐式 (逐段解析) |
| **子域层级验证** | 仅比较后缀 | 检查无嵌套点 | 检查无嵌套点 | 逐段指针遍历 |
| **匹配算法** | 字符串后缀比较 | 前后缀 + 子域检查 | 前后缀 + 子域检查 | 双指针逐字节扫描 |
| **通配符位置限制** | 仅开头 `*.` | 仅开头 `*.` | 仅开头 `*.` | 仅开头连续 `*.` |
| **代码行数** | ~30 行 | ~38 行 | ~27 行 | ~85 行 |

---

## 2. 受影响的文件与代码位置

### 2.1 cert.rs — `is_host_covered()` (L288-L317)

```rust
// src/core/gateway/tls/validation/cert.rs
fn is_host_covered(declared: &str, san_list: &[String]) -> bool {
    let declared_lower = declared.to_lowercase();   // ← 大小写不敏感

    for san in san_list {
        let san_lower = san.to_lowercase();

        if san_lower == declared_lower {
            return true;
        }

        // 通配符匹配: *.example.com 覆盖 api.example.com
        if let Some(san_suffix) = san_lower.strip_prefix("*.") {
            if let Some(dot_pos) = declared_lower.find('.') {
                let declared_suffix = &declared_lower[dot_pos + 1..];
                if san_suffix == declared_suffix {
                    return true;                    // ← 仅比较后缀，不验证子域层级
                }
            }
        }

        // 反向通配符（冗余：已被精确匹配分支覆盖）
        if declared_lower.starts_with("*.") && san_lower.starts_with("*.") 
           && declared_lower == san_lower {
            return true;
        }
    }

    false
}
```

**问题**:
1. L310-L313 的"反向通配符"分支是**死代码**——如果 `declared_lower == san_lower`，L295 的精确匹配已经返回 `true`。
2. 通配符匹配仅比较 `declared_lower[dot_pos+1..]` 与 `san_suffix`，**不检查子域部分是否包含额外的点**。这意味着 SAN `*.example.com` 会错误地匹配 `a.b.example.com`（多级子域）。

**验证** — 让我们用 cert.rs 的逻辑手动推演：
- SAN = `*.example.com`, declared = `a.b.example.com`
- `san_suffix` = `example.com`
- `dot_pos` = 1 (`a.b.example.com` 中第一个点)
- `declared_suffix` = `b.example.com`
- `san_suffix("example.com") != declared_suffix("b.example.com")` → **不匹配** ✓

实际上这里不会有多级通配符 bug，因为 `find('.')` 找到的是第一个点。但这种逻辑与 RFC 6125 的明确规定（通配符仅匹配单个标签）相比，**意图不够清晰**，且缺乏对边界条件的防御性检查。

### 2.2 mtls.rs — `matches_pattern()` (L74-L111)

```rust
// src/core/gateway/tls/validation/mtls.rs
fn matches_pattern(value: &str, pattern: &str) -> bool {
    if pattern == value {                           // ← 大小写敏感！
        return true;
    }

    if let Some(suffix) = pattern.strip_prefix("*.") {
        if pattern.len() < 3 {
            return false;                           // ← 边界保护
        }

        if value.ends_with(suffix) {
            let prefix_len = value.len() - suffix.len();
            if prefix_len == 0 { return false; }
            let prefix = &value[..prefix_len];
            if let Some(subdomain) = prefix.strip_suffix('.') {
                if !subdomain.is_empty() {
                    return !subdomain.contains('.');// ← 严格单级验证
                }
                return false;
            }
        }
    }

    false
}
```

**问题**:
1. **大小写敏感**：`pattern == value` 是精确字节比较。若白名单配置 `*.Example.com`，客户端证书 SAN `test.example.com` 将**无法匹配**。
2. `pattern.len() < 3` 检查**逻辑冗余**：`strip_prefix("*.")` 成功后 `pattern` 至少有 2 个字符(`*.`)，`len < 3` 意味着 pattern 恰好是 `*.`（无后缀域名），此判断正确但可用更清晰的方式表达。

### 2.3 backend_san.rs — `matches_hostname()` (L88-L114)

```rust
// src/core/gateway/tls/validation/backend_san.rs
fn matches_hostname(value: &str, pattern: &str) -> bool {
    if pattern == value {                           // ← 大小写敏感！
        return true;
    }

    if let Some(suffix) = pattern.strip_prefix("*.") {
        if pattern.len() < 3 { return false; }

        if value.ends_with(suffix) {
            let prefix_len = value.len() - suffix.len();
            if prefix_len == 0 { return false; }
            let prefix = &value[..prefix_len];
            if let Some(subdomain) = prefix.strip_suffix('.') {
                if !subdomain.is_empty() {
                    return !subdomain.contains('.');
                }
                return false;
            }
        }
    }

    false
}
```

**问题**:
1. **与 mtls.rs `matches_pattern()` 几乎完全相同的代码**——典型的复制粘贴重复。
2. **同样大小写敏感**。

### 2.4 edgion_tls.rs — `wildcard_match()` (L383-L467)

```rust
// src/types/resources/edgion_tls.rs (简化)
fn wildcard_match(pattern: &str, hostname: &str) -> bool {
    // 双指针逐字节扫描，按域名段匹配
    // 支持多级通配符: *.*.example.com
    // 通配符只能出现在开头（精确段之前）
    // 每个 * 精确匹配一个域名层级
    // ...85 行实现...
}
```

**问题**:
1. 这是**唯一**支持多级通配符（`*.*.example.com`）的实现。
2. 大小写不敏感由调用方 `matches_hostname()` (L358-L376) 通过 `to_lowercase()` 保证，非函数内部处理。
3. 实现复杂度远高于其他三处，但功能也更完整。

---

## 3. 影响分析

### 3.1 安全影响 — 严重级别：高

#### 3.1.1 大小写敏感性差异（RFC 6125 违规）

根据 [RFC 6125 §6.4.1](https://datatracker.ietf.org/doc/html/rfc6125#section-6.4.1)：

> DNS domain names are case-insensitive.

**mtls.rs** 和 **backend_san.rs** 的大小写敏感匹配违反了此规范。具体影响：

| 场景 | 预期行为 | mtls.rs 实际行为 | 后果 |
|------|----------|-----------------|------|
| 白名单 `*.example.com`，客户端证书 SAN = `Test.Example.COM` | 匹配 | **不匹配** | 合法客户端被拒绝（**误拒**） |
| 白名单 `Client.EXAMPLE.com`，客户端证书 CN = `client.example.com` | 匹配 | **不匹配** | 合法客户端被拒绝（**误拒**） |
| 后端证书 SAN = `Backend.Example.COM`，策略要求 `backend.example.com` | 匹配 | **不匹配** | 后端连接失败（**可用性问题**） |

**误拒（false negative）** 在 mTLS 场景中会导致：
- 合法的客户端请求被拒绝，造成服务中断
- 运维人员难以定位问题根因（"配置明明是对的，为什么不匹配？"）
- 在不同 CA 签发证书时，SAN/CN 的大小写可能不一致，导致间歇性故障

#### 3.1.2 多级通配符语义不一致

| 场景 | edgion_tls.rs | mtls.rs / backend_san.rs |
|------|:---:|:---:|
| 配置 `*.*.internal.com` | 匹配 `a.b.internal.com` | **永远不匹配任何值** |

如果用户在 mTLS `allowed_sans` 中配置 `*.*.internal.com`，预期匹配任意两级子域，但 mtls.rs 会将其视为"以 `*.` 开头的模式"并尝试匹配后缀 `*.internal.com`——这永远不会成功，因为 `value.ends_with("*.internal.com")` 几乎不可能为真（证书 SAN 中不会包含 `*` 字符）。

**结果**：配置看似合理但**静默失效**，所有客户端被拒绝。

#### 3.1.3 cert.rs 中的死代码

```rust
// L310-L313: 此分支永远不会被执行
if declared_lower.starts_with("*.") && san_lower.starts_with("*.") 
   && declared_lower == san_lower {
    return true;
}
```

当 `declared_lower == san_lower` 时，L295 已经返回 `true`。死代码虽不影响正确性，但增加了维护负担，且暗示开发者对匹配逻辑的理解可能存在偏差。

### 3.2 可维护性影响

- **修 bug 必须同时修 4 处**：如果在某处发现匹配逻辑 bug，必须记住同步修复所有 4 处实现，遗漏任一处就会引入不一致。
- **测试负担倍增**：每处实现都需要独立的测试集，且测试用例不完全相同——没有统一的边界条件测试覆盖。
- **认知负担**：新开发者阅读代码时，需要理解 4 种不同但相似的实现，并推断它们为何不同——这些差异是有意为之还是无意疏漏？

### 3.3 影响范围

```
请求流入
  │
  ├─ TLS 握手 (SNI 路由) ──→ edgion_tls.rs::wildcard_match()     [大小写不敏感, 多级通配符]
  │
  ├─ 证书加载验证 ──────────→ cert.rs::is_host_covered()           [大小写不敏感, 单级, 有死代码]
  │
  ├─ mTLS 客户端认证 ──────→ mtls.rs::matches_pattern()           [大小写敏感!, 单级]
  │
  └─ 后端 TLS 连接 ────────→ backend_san.rs::matches_hostname()   [大小写敏感!, 单级]
```

**同一个请求的生命周期中，可能经过上述多个匹配阶段，每个阶段的匹配规则不同。**

---

## 4. 根因分析

### 4.1 直接原因

四个匹配函数在不同时间、由可能不同的开发者（或同一开发者在不同上下文中）独立实现，缺乏**共享的通配符匹配抽象**。

### 4.2 根本原因

1. **缺少领域工具层（domain utility layer）**：项目中没有 `src/core/utils/hostname.rs` 或类似的共享匹配模块。
2. **缺少编码规范约束**：没有明确规定"所有主机名匹配必须大小写不敏感"的项目级规范。
3. **复制粘贴驱动开发**：`mtls.rs` 和 `backend_san.rs` 的代码几乎完全一致，是典型的复制粘贴产物。

---

## 5. 修复建议：提取统一的 `HostMatcher` 工具

### 5.1 设计目标

1. **单一实现**：所有通配符匹配逻辑收敛到一个模块。
2. **RFC 6125 合规**：默认大小写不敏感。
3. **可配置策略**：通过枚举控制是否允许多级通配符。
4. **零成本抽象**：使用 `#[inline]` 和栈分配，避免堆分配。

### 5.2 建议的模块位置

```
src/core/gateway/tls/
├── mod.rs
├── host_match.rs          ← 新增：统一匹配模块
└── validation/
    ├── cert.rs            ← 改为调用 host_match
    ├── mtls.rs            ← 改为调用 host_match
    └── backend_san.rs     ← 改为调用 host_match

src/types/resources/
    └── edgion_tls.rs      ← 改为调用 host_match
```

### 5.3 统一实现代码示例

```rust
// src/core/gateway/tls/host_match.rs

/// 通配符匹配策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WildcardPolicy {
    /// 仅允许单级通配符: `*.example.com`
    /// 适用于 RFC 6125 严格场景（证书 SAN 验证、mTLS 白名单）
    SingleLevel,
    /// 允许多级通配符: `*.*.example.com`
    /// 适用于路由匹配等宽松场景
    MultiLevel,
}

/// 检查 `hostname` 是否匹配 `pattern`。
///
/// 行为特征:
/// - 大小写不敏感（RFC 6125 §6.4.1）
/// - `*` 仅能出现在模式开头，以 `*.` 形式
/// - 每个 `*` 精确匹配一个域名标签（label）
/// - `SingleLevel` 策略下 `*.*.example.com` 被视为无效模式（返回 false）
#[inline]
pub fn hostname_matches(hostname: &str, pattern: &str, policy: WildcardPolicy) -> bool {
    // 快速路径：精确匹配（大小写不敏感）
    if hostname.eq_ignore_ascii_case(pattern) {
        return true;
    }

    // 无通配符时，精确匹配已失败，直接返回
    if !pattern.starts_with("*.") {
        return false;
    }

    match policy {
        WildcardPolicy::SingleLevel => single_level_wildcard_match(hostname, pattern),
        WildcardPolicy::MultiLevel => multi_level_wildcard_match(hostname, pattern),
    }
}

/// 单级通配符匹配: `*.example.com` 仅匹配 `foo.example.com`
fn single_level_wildcard_match(hostname: &str, pattern: &str) -> bool {
    // pattern = "*.suffix"，提取 suffix
    let suffix = match pattern.strip_prefix("*.") {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };

    // 单级策略下，后缀本身不能包含通配符
    if suffix.contains('*') {
        return false;
    }

    // hostname 必须以 ".suffix" 结尾（大小写不敏感）
    let hostname_bytes = hostname.as_bytes();
    let suffix_bytes = suffix.as_bytes();

    // hostname 至少需要 "x." + suffix 的长度（至少 2 + suffix_len）
    if hostname_bytes.len() < suffix_bytes.len() + 2 {
        return false;
    }

    // 检查 hostname 以 ".suffix" 结尾（大小写不敏感）
    let hostname_suffix_start = hostname_bytes.len() - suffix_bytes.len();
    if hostname_bytes[hostname_suffix_start - 1] != b'.' {
        return false;
    }
    if !hostname[hostname_suffix_start..].eq_ignore_ascii_case(suffix) {
        return false;
    }

    // 提取子域部分（hostname_suffix_start - 1 之前的部分），确保不含点
    let subdomain = &hostname[..hostname_suffix_start - 1];
    !subdomain.is_empty() && !subdomain.contains('.')
}

/// 多级通配符匹配: `*.*.example.com` 匹配 `a.b.example.com`
fn multi_level_wildcard_match(hostname: &str, pattern: &str) -> bool {
    let pattern_labels: Vec<&str> = pattern.split('.').collect();
    let hostname_labels: Vec<&str> = hostname.split('.').collect();

    // 标签数量必须相同
    if pattern_labels.len() != hostname_labels.len() {
        return false;
    }

    let mut seen_exact = false;
    for (p_label, h_label) in pattern_labels.iter().zip(hostname_labels.iter()) {
        if *p_label == "*" {
            // 通配符只能出现在精确标签之前
            if seen_exact {
                return false;
            }
            // 通配符必须匹配至少一个字符
            if h_label.is_empty() {
                return false;
            }
        } else {
            seen_exact = true;
            // 精确标签大小写不敏感比较
            if !p_label.eq_ignore_ascii_case(h_label) {
                return false;
            }
        }
    }

    true
}

/// 便捷函数：严格 RFC 6125 匹配（用于证书/mTLS 验证）
#[inline]
pub fn hostname_matches_strict(hostname: &str, pattern: &str) -> bool {
    hostname_matches(hostname, pattern, WildcardPolicy::SingleLevel)
}

/// 便捷函数：宽松匹配（用于路由/SNI 匹配）
#[inline]
pub fn hostname_matches_relaxed(hostname: &str, pattern: &str) -> bool {
    hostname_matches(hostname, pattern, WildcardPolicy::MultiLevel)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== 单级通配符测试 ==========

    #[test]
    fn test_exact_match_case_insensitive() {
        assert!(hostname_matches_strict("Example.COM", "example.com"));
        assert!(hostname_matches_strict("example.com", "EXAMPLE.COM"));
    }

    #[test]
    fn test_single_wildcard_basic() {
        assert!(hostname_matches_strict("foo.example.com", "*.example.com"));
        assert!(hostname_matches_strict("bar.example.com", "*.example.com"));
    }

    #[test]
    fn test_single_wildcard_case_insensitive() {
        assert!(hostname_matches_strict("Foo.Example.COM", "*.example.com"));
        assert!(hostname_matches_strict("foo.example.com", "*.Example.COM"));
    }

    #[test]
    fn test_single_wildcard_rejects_multi_level() {
        assert!(!hostname_matches_strict("a.b.example.com", "*.example.com"));
    }

    #[test]
    fn test_single_wildcard_rejects_base_domain() {
        assert!(!hostname_matches_strict("example.com", "*.example.com"));
    }

    #[test]
    fn test_single_wildcard_rejects_multi_wildcard_pattern() {
        // 单级策略下，*.*.example.com 被视为无效模式
        assert!(!hostname_matches_strict("a.b.example.com", "*.*.example.com"));
    }

    #[test]
    fn test_invalid_patterns() {
        assert!(!hostname_matches_strict("foo.example.com", "*."));
        assert!(!hostname_matches_strict("foo.example.com", "*"));
        assert!(!hostname_matches_strict("", "*.example.com"));
    }

    // ========== 多级通配符测试 ==========

    #[test]
    fn test_multi_wildcard_double() {
        assert!(hostname_matches_relaxed("a.b.example.com", "*.*.example.com"));
        assert!(!hostname_matches_relaxed("a.example.com", "*.*.example.com"));
        assert!(!hostname_matches_relaxed("a.b.c.example.com", "*.*.example.com"));
    }

    #[test]
    fn test_multi_wildcard_triple() {
        assert!(hostname_matches_relaxed("a.b.c.example.com", "*.*.*.example.com"));
        assert!(!hostname_matches_relaxed("a.b.example.com", "*.*.*.example.com"));
    }

    #[test]
    fn test_multi_wildcard_case_insensitive() {
        assert!(hostname_matches_relaxed("A.B.Example.COM", "*.*.example.com"));
    }

    #[test]
    fn test_wildcard_not_in_middle() {
        assert!(!hostname_matches_relaxed("foo.bar.example.com", "foo.*.example.com"));
        assert!(!hostname_matches_relaxed("a.b.c.com", "*.b.*.com"));
    }

    // ========== 边界条件测试 ==========

    #[test]
    fn test_empty_inputs() {
        assert!(!hostname_matches_strict("", ""));
        assert!(!hostname_matches_strict("", "*.example.com"));
        assert!(!hostname_matches_strict("foo.example.com", ""));
    }

    #[test]
    fn test_trailing_dot_not_normalized() {
        // 暂不处理 FQDN 尾部点，保持当前行为
        assert!(!hostname_matches_strict("foo.example.com.", "*.example.com"));
    }
}
```

### 5.4 各调用方迁移示例

#### cert.rs 迁移

```rust
// Before:
fn is_host_covered(declared: &str, san_list: &[String]) -> bool {
    let declared_lower = declared.to_lowercase();
    for san in san_list {
        let san_lower = san.to_lowercase();
        if san_lower == declared_lower { return true; }
        if let Some(san_suffix) = san_lower.strip_prefix("*.") { /* ... 30行 ... */ }
        // 死代码...
    }
    false
}

// After:
use crate::core::gateway::tls::host_match::hostname_matches_strict;

fn is_host_covered(declared: &str, san_list: &[String]) -> bool {
    san_list.iter().any(|san| hostname_matches_strict(declared, san))
}
```

#### mtls.rs 迁移

```rust
// Before:
fn matches_pattern(value: &str, pattern: &str) -> bool { /* ... 38行, 大小写敏感 ... */ }

// After:
use crate::core::gateway::tls::host_match::hostname_matches_strict;

fn matches_pattern(value: &str, pattern: &str) -> bool {
    hostname_matches_strict(value, pattern)
}
```

#### backend_san.rs 迁移

```rust
// Before:
fn matches_hostname(value: &str, pattern: &str) -> bool { /* ... 27行, 与 mtls.rs 几乎相同 ... */ }

// After:
use crate::core::gateway::tls::host_match::hostname_matches_strict;

fn matches_hostname(value: &str, pattern: &str) -> bool {
    hostname_matches_strict(value, pattern)
}
```

#### edgion_tls.rs 迁移

```rust
// Before:
impl EdgionTls {
    pub fn matches_hostname(&self, hostname: &str) -> bool {
        let hostname_lower = hostname.to_lowercase();
        for host in &self.spec.hosts {
            let host_lower = host.to_lowercase();
            if host_lower == hostname_lower { return true; }
            if host_lower.starts_with('*') && Self::wildcard_match(&host_lower, &hostname_lower) {
                return true;
            }
        }
        false
    }

    fn wildcard_match(pattern: &str, hostname: &str) -> bool { /* ... 85行 ... */ }
}

// After:
use crate::core::gateway::tls::host_match::hostname_matches_relaxed;

impl EdgionTls {
    pub fn matches_hostname(&self, hostname: &str) -> bool {
        self.spec.hosts.iter().any(|host| hostname_matches_relaxed(hostname, host))
    }
    // 删除 wildcard_match() 方法
}
```

---

## 6. 测试建议

### 6.1 统一测试矩阵

建议为 `host_match.rs` 建立涵盖以下维度的测试矩阵：

| 维度 | 用例 |
|------|------|
| **精确匹配** | 完全相同、大小写不同、完全不同 |
| **单级通配符** | 正常匹配、多级被拒、基域名被拒、空子域被拒 |
| **多级通配符** | 2级、3级、层级不匹配、中间通配符被拒 |
| **大小写** | pattern 大写 + hostname 小写、反之亦然、混合大小写 |
| **边界条件** | 空字符串、仅 `*`、仅 `*.`、超长域名、单字符子域 |
| **非法模式** | `*-api.example.com`、`api-*.example.com`、`foo.*.com` |
| **RFC 合规** | 尾部点(FQDN)、国际化域名(IDN punycode) |

### 6.2 回归测试策略

1. **保留现有测试**：迁移后，原有四个文件中的测试应继续通过（作为集成测试保留）。
2. **新增对比测试**：编写参数化测试，确保新的 `hostname_matches` 在所有旧测试输入上产生相同结果（除了预期修复的大小写敏感 bug）。
3. **Property-based 测试**：使用 `proptest` 或 `quickcheck` 生成随机主机名和模式，验证不变式：
   - `hostname_matches(h, h, _)` 总是 `true`（自匹配）
   - `hostname_matches(h, p, SingleLevel) == true` → `hostname_matches(h, p, MultiLevel) == true`（宽松策略是严格策略的超集）

### 6.3 建议的回归测试用例（重点：大小写敏感修复）

```rust
#[test]
fn regression_case_insensitive_mtls() {
    // 修复前: mtls.rs matches_pattern 大小写敏感，此断言会失败
    assert!(hostname_matches_strict("Test.Example.COM", "*.example.com"));
    assert!(hostname_matches_strict("CLIENT.INTERNAL.ORG", "client.internal.org"));
}

#[test]
fn regression_case_insensitive_backend_san() {
    // 修复前: backend_san.rs matches_hostname 大小写敏感，此断言会失败
    assert!(hostname_matches_strict("Backend.Example.COM", "backend.example.com"));
    assert!(hostname_matches_strict("svc.CLUSTER.LOCAL", "*.cluster.local"));
}

#[test]
fn regression_multi_level_pattern_in_strict_mode() {
    // 确保严格模式下多级通配符不会静默失效，而是明确拒绝
    assert!(!hostname_matches_strict("a.b.internal.com", "*.*.internal.com"));
}
```

---

## 7. 实施步骤

| 阶段 | 步骤 | 风险 |
|------|------|------|
| 1 | 创建 `src/core/gateway/tls/host_match.rs`，实现 `hostname_matches` | 低 — 纯新增代码 |
| 2 | 为 `host_match.rs` 编写完整测试集 | 低 |
| 3 | 迁移 `cert.rs::is_host_covered()` | 中 — 修复死代码，行为微调 |
| 4 | 迁移 `mtls.rs::matches_pattern()` | **高** — 大小写行为变更，需回归测试 |
| 5 | 迁移 `backend_san.rs::matches_hostname()` | **高** — 同上 |
| 6 | 迁移 `edgion_tls.rs::wildcard_match()` | 中 — 功能等价，算法替换 |
| 7 | 删除旧的私有匹配函数 | 低 — 编译器会检查未使用代码 |
| 8 | 全量集成测试 + 手动 mTLS 测试 | — |

---

## 8. 总结

| 维度 | 当前状态 | 目标状态 |
|------|----------|----------|
| 实现数量 | 4 处独立实现 | 1 处统一实现 + 4 处调用 |
| 大小写处理 | 2/4 敏感, 2/4 不敏感 | 全部不敏感 (RFC 6125) |
| 多级通配符 | 1/4 支持 | 按策略可选 (严格/宽松) |
| 测试覆盖 | 各自独立, 用例不完整 | 统一测试矩阵 + 回归测试 |
| 代码行数 | ~180 行（4处总计） | ~100 行（1处 + 便捷包装） |
| 维护成本 | 修 1 个 bug 需同步 4 处 | 修 1 处即生效 |

这是一个**安全相关的重构**，建议在下一个 sprint 中优先处理，尤其是 mTLS 和 backend SAN 的大小写敏感问题需要尽快修复。
