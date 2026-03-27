# BackendTLSPolicy SubjectAltNames 运行时验证

## 优先级: P1
## 工作量: 中（预计 2-3 天）
## 类型: 功能缺失

---

## 问题描述

BackendTLSPolicy 的 `validation.subjectAltNames` 字段已在结构体中定义（支持 Hostname 和 URI 类型），
但在 TLS 握手时未实际校验后端证书的 SAN。这意味着即使配置了 subjectAltNames，也不会验证后端证书是否匹配。

这对 SPIFFE/SPIRE 集成场景（使用 URI 类型 SAN 如 `spiffe://cluster.local/ns/default/sa/myapp`）尤其重要。

## 当前状态

### 已有
- `SubjectAltName` 结构体定义：`type` (Hostname/URI) + `name`
- `BackendTLSPolicyValidation.subject_alt_names: Option<Vec<SubjectAltName>>`
- Controller 解析并传递 subjectAltNames 到 Gateway
- `cert_extractor.rs` 中有 SAN 提取逻辑（用于前端 mTLS）

### 缺失
- Gateway 在与后端建立 TLS 连接时，未将 policy 中的 subjectAltNames 与后端证书的实际 SAN 进行比对
- 未区分 Hostname 和 URI 类型的 SAN 验证逻辑

## 涉及文件

### 数据模型（已有）
1. `src/types/resources/backend_tls_policy.rs:121-147` — SubjectAltName 定义

### 需要修改
2. `src/core/gateway/tls/runtime/backend/` — 后端 TLS 连接建立逻辑
3. `src/core/gateway/tls/validation/` — TLS 验证模块
4. `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` — upstream peer TLS 配置

### 可复用
5. `src/core/gateway/tls/runtime/backend/cert_extractor.rs` — SAN 提取逻辑（已有，用于前端）
6. `src/core/gateway/tls/validation/mtls.rs` — `validate_san_whitelist()` 可作为参考

## 实现建议

1. 在后端 TLS 连接建立时（upstream peer），获取 BackendTLSPolicy 中的 subjectAltNames 配置
2. 在 TLS 握手回调（或握手后验证）中，提取后端证书的 SAN 列表
3. 按类型匹配：
   - **Hostname 类型**：与证书 SAN 中的 DNS Name 条目比对
   - **URI 类型**：与证书 SAN 中的 URI 条目比对（SPIFFE ID 场景）
4. 如果配置了 subjectAltNames 但后端证书不匹配任何一项，拒绝连接
5. 如果未配置 subjectAltNames（None），保持现有行为（仅 hostname 验证）

### 参考 cert_extractor.rs 中已有的 SAN 提取代码
现有的 `validate_san_whitelist()` 在 `mtls.rs` 中处理前端证书 SAN 验证，
可以复用类似逻辑，将其泛化为支持后端证书 SAN 验证。

## 参考 Skills

- `skills/01-architecture/02-gateway/SKILL.md` — Gateway TLS 架构
- `skills/09-misc/debugging-tls-gateway.md` — TLS 调试指南
- `skills/05-testing/00-integration-testing.md` — 集成测试

## 验证标准

- 配置 Hostname 类型 SAN：后端证书 DNS SAN 匹配时连接成功，不匹配时拒绝
- 配置 URI 类型 SAN：后端证书 URI SAN 匹配时连接成功（SPIFFE ID 场景）
- 未配置 subjectAltNames 时行为不变
- 混合 Hostname + URI SAN：任一匹配即可
- `cargo check --all-targets` 通过
- `cargo clippy --all-targets` 通过
- 集成测试覆盖 Hostname 和 URI 两种 SAN 类型
