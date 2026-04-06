# Edgion 依赖升级行动计划

> 创建日期: 2026-03-30
> 关联报告: [security-audit-report.md](./security-audit-report.md)

---

## 行动项清单

### ✅ Action 1: 执行 cargo update 修复已知安全漏洞

**优先级**: P0 - 紧急
**工作量**: 5 分钟
**风险**: 极低（semver 兼容范围内的 patch 升级）

**步骤**:
```bash
# 1. 执行更新
cargo update

# 2. 验证安全漏洞已修复
cargo audit

# 3. 构建测试
cargo build
cargo test
```

**预期修复**:
- [x] aws-lc-sys 0.36.0 → 0.39.1（5 条 advisory）
- [x] quinn-proto 0.11.13 → 0.11.14（1 条 advisory）
- [x] rustls-webpki 0.103.9 → 0.103.10（1 条 advisory）

**验证标准**: `cargo audit` 仅剩 protobuf 相关的 1 条漏洞和 3 条 unmaintained 警告。

---

### ✅ Action 2: 将 serde_yaml 替换为 yaml_serde

**优先级**: P1 - 高
**工作量**: 15 分钟
**风险**: 极低（API 100% 兼容的官方 fork）

**修改内容**:

Cargo.toml 变更:
```toml
# 替换前
serde_yaml = "0.9"

# 替换后
serde_yaml = { package = "yaml_serde", version = "0.10" }
```

**验证步骤**:
```bash
cargo build
cargo test
```

**说明**: 使用 `package` 重命名方式可以保持全部 35 个源文件中的 `use serde_yaml::` 导入不变。

---

### ⏳ Action 3: 跟踪 Pingora 上游修复

**优先级**: P2 - 中（有缓解措施）
**工作量**: 持续跟踪
**风险**: 中（需等待上游）

**跟踪事项**:

| Issue | 描述 | 跟踪地址 |
|-------|------|----------|
| protobuf v2 DoS | prometheus 依赖的 protobuf v2 存在不受控递归 | [pingora#552](https://github.com/cloudflare/pingora/issues/552) |
| prometheus optional | 社区要求将 prometheus 改为 optional 依赖 | [pingora#560](https://github.com/cloudflare/pingora/issues/560) |

**临时缓解措施**:
1. 确保 prometheus metrics 端点 (`/metrics`) 不暴露到公网
2. 在网关/防火墙层面限制对 metrics 端点的访问来源
3. 定期检查 pingora 新版本发布

---

### 📋 Action 4: 评估可选的大版本升级（非安全必需）

**优先级**: P3 - 低
**工作量**: 需专项评估

| 库 | 升级路径 | 评估要点 |
|----|---------|---------|
| boring/boring-sys | 4.x → 5.x | 检查 BoringSSL API 变更，评估 TLS callback 代码兼容性 |
| jsonschema | 0.40 → 0.45 | 检查 Schema 验证 API 是否有 breaking change |
| toml | 0.9 → 1.1 | 检查配置文件读写代码兼容性 |

---

## 时间线建议

| 阶段 | 内容 | 预计时间 |
|------|------|---------|
| 本周 | 完成 Action 1 + Action 2 | 0.5h |
| 持续 | 跟踪 Action 3 上游进展 | 每两周检查一次 |
| 下个迭代 | 评估 Action 4 大版本升级需求 | 按需 |
