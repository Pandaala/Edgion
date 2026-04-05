# Step 04: 精确 status reason 值 ✅

> **Gap 4（影响状态匹配）**：conformance 测试检查精确的 reason 值，当前使用的值不符合标准。

## Conformance 要求的 reason 值

| 场景 | 期望 reason | 修复前 | 修复后 |
|------|-------------|--------|--------|
| CA 证书数据无效/解析失败 | `NoValidCACertificate` | `Invalid` | ✅ `NoValidCACertificate` |
| caCertificateRef 引用不存在 | `InvalidCACertificateRef` | `BackendNotFound` | ✅ `InvalidCACertificateRef` |
| caCertificateRef kind 不支持 | `InvalidKind` | `BackendNotFound` | ✅ `InvalidKind` |
| 冲突（同一 target 多个 policy） | `Conflicted` | 完全缺失 | ✅ `Conflicted`（step-03） |
| 所有 ref 解析成功 | `ResolvedRefs` | `ResolvedRefs` | ✅ 已正确 |
| Policy 有效且被接受 | `Accepted` | `Accepted` | ✅ 已正确 |

## 实现方案

### 核心变更：typed error enum

引入 `BtpResolvedRefError` 枚举替代原有的 `Vec<String>`，区分三类错误：

```
BtpResolvedRefError::InvalidKind(msg)              → caCertificateRef kind 不支持
BtpResolvedRefError::InvalidCACertificateRef(msg)   → ref 不存在或 CA 数据缺失
BtpResolvedRefError::Other(msg)                     → 客户端证书等其他 ref 错误
```

### Accepted condition reason 优先级

`Invalid` > `NoValidCACertificate` > `Conflicted` > `Accepted`

- validation error（如互斥字段）→ `Reason=Invalid`
- CA ref 相关错误（InvalidKind 或 InvalidCACertificateRef）→ `Reason=NoValidCACertificate`
- 冲突检测失败 → `Reason=Conflicted`
- 通过所有检查 → `Reason=Accepted`

### ResolvedRefs condition reason 优先级

`InvalidKind` > `InvalidCACertificateRef` > `BackendNotFound`

由 `pick_btp_resolved_refs_reason()` 函数实现，与 route handler 中 `pick_resolved_refs_reason()` 同模式。

### 新增 reason 常量

`status_utils.rs` 中新增：
- `NO_VALID_CA_CERTIFICATE = "NoValidCACertificate"`
- `INVALID_CA_CERTIFICATE_REF = "InvalidCACertificateRef"`

## 变更文件

| 文件 | 变更 | 说明 |
|------|------|------|
| `status_utils.rs` | 修改 | 新增 2 个 BackendTLSPolicy-specific reason 常量 |
| `handlers/backend_tls_policy.rs` | 修改 | 新增 `BtpResolvedRefError` 枚举 + `pick_btp_resolved_refs_reason()` + 重构 `update_policy_status` |

## 验证

- `cargo check --all-targets` — 通过
- `cargo clippy --all-targets` — 无新 warning
- `cargo test --all` — 1644 passed, 0 failed
