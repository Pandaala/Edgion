# Edgion 依赖安全审计报告

> 审计日期: 2026-03-30
> 工具: `cargo audit` (RustSec Advisory DB), `cargo update --dry-run`
> 扫描范围: Cargo.lock 中 606 个 crate 依赖

---

## 一、概要

| 类别 | 数量 | 说明 |
|------|------|------|
| 安全漏洞 (vulnerability) | **5 个 CVE / 8 条 advisory** | 涉及 4 个 crate |
| 未维护警告 (unmaintained) | **3 条** | daemonize, derivative, rustls-pemfile |
| 已弃用库 (deprecated) | **1 个** | serde_yaml |

### 风险等级总览

| 严重等级 | 数量 | 涉及 crate |
|----------|------|------------|
| 🔴 HIGH (≥7.0) | 4 | aws-lc-sys (×3), quinn-proto (×1) |
| 🟡 MEDIUM (5.0-6.9) | 1 | aws-lc-sys (×1) |
| 🟠 未评分但有影响 | 3 | aws-lc-sys (×1), rustls-webpki (×1), protobuf (×1) |

---

## 二、必须升级的安全漏洞（详细分析）

### 2.1 aws-lc-sys v0.36.0 → ≥ v0.39.1 ⚠️ 最高优先级

**当前版本**: 0.36.0
**修复版本**: ≥ 0.39.0（建议升级到 0.39.1）

| Advisory ID | 标题 | 严重性 | 说明 |
|-------------|------|--------|------|
| RUSTSEC-2026-0048 | CRL Distribution Point 作用域检查逻辑错误 | **7.4 HIGH** | CRL 证书吊销列表分发点范围匹配存在逻辑缺陷，可能导致吊销证书被错误接受 |
| RUSTSEC-2026-0047 | PKCS7_verify 签名验证绕过 | **7.5 HIGH** | PKCS7 签名验证存在绕过漏洞，攻击者可伪造签名 |
| RUSTSEC-2026-0046 | PKCS7_verify 证书链验证绕过 | **7.5 HIGH** | PKCS7 证书链验证存在绕过漏洞，攻击者可使用无效证书链 |
| RUSTSEC-2026-0045 | AES-CCM 标签验证时序侧信道 | **5.9 MEDIUM** | AES-CCM 认证标签比较存在时序侧信道，可被利用还原 tag 值 |
| RUSTSEC-2026-0044 | X.509 Name Constraints 通配符/Unicode CN 绕过 | **未评分** | 通过通配符或 Unicode 的 Common Name 绕过 X.509 命名约束 |

**影响范围**: aws-lc-sys 是 TLS 基础设施的底层密码学库，通过 aws-lc-rs → rustls → 几乎所有网络组件 传导影响，涵盖:
- rustls (TLS 核心)
- tokio-rustls → tonic, axum-server, fred, ldap3
- reqwest, kube-client, instant-acme
- pingora-rustls → pingora-core/proxy
- x509-parser, rcgen (证书处理)

**是否可升级**: ✅ **可以升级，推荐立即执行**

**升级方式**: 这是传递依赖，只需执行 `cargo update` 即可自动从 0.36.0 升级到 0.39.1（semver 兼容范围内）。`cargo update --dry-run` 已确认可用。

**升级影响分析**:
- aws-lc-sys 升级属于 semver patch/minor 级别（0.x 系列），但作为密码学底层库，其 API 变化通常不影响上层使用
- aws-lc-rs 同步从 1.15.3 → 1.16.2
- 无需修改任何 Cargo.toml 或项目代码
- **风险**: 极低。密码学行为更安全，可能导致之前错误接受的无效证书被正确拒绝（这是预期行为修正）

---

### 2.2 quinn-proto v0.11.13 → v0.11.14 ⚠️ 高优先级

**当前版本**: 0.11.13
**修复版本**: ≥ 0.11.14

| Advisory ID | 标题 | 严重性 |
|-------------|------|--------|
| RUSTSEC-2026-0037 | Quinn 端点拒绝服务攻击 | **8.7 HIGH** |

**漏洞详情**: QUIC 协议参数解析时因 VarInt 解码缺少错误处理导致 panic，远程攻击者可发送恶意数据包导致服务崩溃。

**影响范围**: quinn-proto → quinn → reqwest → edgion（HTTP/3 QUIC 传输层）

**是否可升级**: ✅ **可以升级，推荐立即执行**

**升级方式**: `cargo update` 自动从 0.11.13 升级到 0.11.14（semver 兼容）。

**升级影响分析**:
- 这是一个纯 bugfix patch 级别升级
- 仅修复了参数解析时的 unwrap panic
- 无需修改任何代码
- **风险**: 极低

---

### 2.3 rustls-webpki v0.103.9 → v0.103.10 ⚠️ 高优先级

**当前版本**: 0.103.9
**修复版本**: ≥ 0.103.10

| Advisory ID | 标题 | 严重性 |
|-------------|------|--------|
| RUSTSEC-2026-0049 | CRL Distribution Point 匹配逻辑错误导致 CRL 不被视为权威 | **未评分** |

**漏洞详情**: CRL（证书吊销列表）分发点的匹配逻辑有缺陷，导致有效的 CRL 可能不被认为是权威的，从而使已吊销的证书未被正确拒绝。

**影响范围**: rustls-webpki → rustls → 全部 TLS 相关组件（与 aws-lc-sys 影响范围重叠）

**是否可升级**: ✅ **可以升级，推荐立即执行**

**升级方式**: `cargo update` 自动从 0.103.9 升级到 0.103.10。

**升级影响分析**:
- patch 级别的 bugfix
- 修复后 CRL 检查行为更正确，已吊销证书将被正确拒绝
- 无需修改任何代码
- **风险**: 极低。注意如果之前依赖了"错误地接受已吊销证书"的行为，修复后会正确拒绝

---

### 2.4 protobuf v2.28.0 → ≥ v3.7.2 ⚠️ 中优先级（需等待上游）

**当前版本**: 2.28.0
**修复版本**: ≥ 3.7.2

| Advisory ID | 标题 | 严重性 |
|-------------|------|--------|
| RUSTSEC-2024-0437 | 不受控递归导致崩溃 | **未评分** |

**漏洞详情**: protobuf 解析器在处理深度嵌套的 protobuf 消息时存在不受控递归，可导致栈溢出崩溃。攻击者可构造特殊的 protobuf 消息触发 DoS。

**影响范围**: protobuf v2.28.0 → prometheus v0.13.4 → pingora-core v0.8.0

**是否可升级**: ❌ **当前无法直接升级**

**原因分析**:
1. protobuf 是 pingora-core 的间接依赖，通过 `prometheus v0.13.4` 引入
2. prometheus 0.13.4 依赖 protobuf v2.x，无法升级到 3.x（大版本 breaking change）
3. Cloudflare 已知此问题（[GitHub Issue #552](https://github.com/cloudflare/pingora/issues/552)），社区要求将 prometheus 改为 optional 依赖
4. pingora 0.8.0 是当前最新版本（2026-03-02 发布），尚未修复此问题

**缓解措施**:
- 该漏洞需要攻击者能向 prometheus 指标端点发送恶意 protobuf 数据
- 如果 prometheus 指标端点未暴露到不可信网络，实际风险较低
- 建议关注 pingora 后续版本更新，在上游修复后及时升级
- 可考虑在网络层限制对 metrics 端点的访问

---

## 三、未维护库警告（Unmaintained）

### 3.1 daemonize v0.5.0

| 字段 | 值 |
|------|-----|
| Advisory | RUSTSEC-2025-0069 |
| 依赖链 | pingora-core → edgion |
| 是否可替换 | ❌ 无法直接替换，属于 pingora-core 内部依赖 |

**分析**: daemonize 用于进程守护化，功能稳定且简单，短期内不会产生安全风险。需等待 pingora 上游处理。

### 3.2 derivative v2.2.0

| 字段 | 值 |
|------|-----|
| Advisory | RUSTSEC-2024-0388 |
| 依赖链 | pingora-core, pingora-load-balancing → edgion |
| 是否可替换 | ❌ 无法直接替换，属于 pingora 内部依赖 |

**分析**: derivative 是 derive 宏库，功能已完善，推荐替代品为 `educe`。同样需等待 pingora 上游迁移。

### 3.3 rustls-pemfile v2.2.0

| 字段 | 值 |
|------|-----|
| Advisory | RUSTSEC-2025-0134 |
| 依赖链 | tonic 0.12.3 → edgion, pingora-rustls → pingora-core |
| 是否可替换 | ❌ 无法直接替换，属于 tonic 和 pingora 的依赖 |

**分析**: rustls-pemfile 功能已被合并到 rustls-pki-types crate 中。项目代码中未直接使用此库。需等待 tonic 和 pingora 上游升级。

---

## 四、已弃用库

### 4.1 serde_yaml v0.9.34 (deprecated)

| 字段 | 值 |
|------|-----|
| 状态 | 原作者已停止维护并归档 |
| 推荐替代 | `yaml_serde` (官方维护的 fork) |
| 项目使用情况 | **35 个文件**中有引用 |
| 是否可替换 | ✅ 可以替换，API 完全兼容 |

**迁移方式（零代码改动）**:

```toml
# 旧
serde_yaml = "0.9"

# 新（通过 package 重命名，无需修改 use 语句）
serde_yaml = { package = "yaml_serde", version = "0.10" }
```

**影响分析**:
- yaml_serde 是 serde_yaml 的官方 fork，API 100% 兼容
- 使用 `package` 重命名后无需修改任何 `use serde_yaml::` 导入
- **风险**: 极低，建议优先处理

---

## 五、升级行动计划

### 第一优先级：立即执行 `cargo update`（零风险）

以下漏洞可通过一次 `cargo update` 全部修复，**无需修改任何代码或 Cargo.toml**：

| Crate | 升级路径 | 修复的漏洞 |
|-------|----------|-----------|
| aws-lc-sys | 0.36.0 → 0.39.1 | RUSTSEC-2026-0044/0045/0046/0047/0048 (×5) |
| quinn-proto | 0.11.13 → 0.11.14 | RUSTSEC-2026-0037 (×1) |
| rustls-webpki | 0.103.9 → 0.103.10 | RUSTSEC-2026-0049 (×1) |
| rustls | 0.23.36 → 0.23.37 | - |

**执行命令**:
```bash
cargo update
cargo audit
```

**预期结果**: 修复 7/8 条安全 advisory，仅剩 protobuf 相关的 1 条。

### 第二优先级：替换 serde_yaml（低风险）

修改 `Cargo.toml`：
```toml
serde_yaml = { package = "yaml_serde", version = "0.10" }
```

### 第三优先级：关注上游更新（无法立即处理）

| 问题 | 阻塞方 | 跟踪链接 |
|------|--------|----------|
| protobuf v2 不受控递归 | pingora-core (prometheus 依赖) | [cloudflare/pingora#552](https://github.com/cloudflare/pingora/issues/552) |
| daemonize 未维护 | pingora-core | 等待 pingora 下一版本 |
| derivative 未维护 | pingora-core/load-balancing | 等待 pingora 下一版本 |
| rustls-pemfile 未维护 | tonic 0.12, pingora-rustls | 等待 tonic/pingora 升级 |

---

## 六、其他需关注的版本更新

以下非安全性更新，但 `cargo update --dry-run` 显示有新版本可用（semver 兼容）：

| Crate | 当前 → 可用 | 说明 |
|-------|------------|------|
| tokio | 1.49.0 → 1.50.0 | 异步运行时，建议定期跟进 |
| clap | 4.5.54 → 4.6.0 | CLI 框架 minor 升级 |
| chrono | 0.4.43 → 0.4.44 | 时间库 patch 更新 |
| futures | 0.3.31 → 0.3.32 | 异步基础库 patch 更新 |
| serde | 1.0.228 (已最新) | - |

> 以上更新均包含在 `cargo update` 中，会一并升级。

---

## 七、Cargo.toml 中可选的大版本升级（需评估）

`cargo update --dry-run` 还提示以下库有超出当前 semver 范围的新大版本：

| Crate | 当前版本 | 最新大版本 | 评估 |
|-------|---------|-----------|------|
| boring / boring-sys | 4.x | 5.0.2 | 涉及 BoringSSL 绑定大版本变更，API 可能不兼容，需专项评估 |
| jsonschema | 0.40.x | 0.45.0 | 可能有 API 变化，需检查 Schema 验证逻辑 |
| toml | 0.9.x | 1.1.0 | 大版本升级，需检查 API 兼容性 |

> 以上大版本升级不包含已知安全漏洞，可在后续版本迭代中按需评估。
