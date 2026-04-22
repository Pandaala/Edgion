# Webhook Key Resolver 响应无签名校验：依赖 TLS 是行业模式

## 结论

`webhook/resolver.rs` 不对 webhook 服务的 HTTP 响应做 HMAC / 签名校验是**刻意的架构取舍**——与业界"主动调外部服务"类场景（OAuth Introspection、OPA ExtAuthz）一致，依赖 TLS + 合法 CA 作为唯一防线。**不应**要求引入 `responseSignature` 配置字段或强制签名校验。

## 原因

1. **语义方向与传统 webhook 倒置**：
   - GitHub / Stripe / Slack 的"webhook 签名"是 **sender→receiver 方向**（sender 签名、receiver 验），其存在理由是 receiver 无法主动验证 sender 身份
   - Edgion `WebhookKeyGet` 是**反向**——Edgion 主动发起 HTTP 调用到 operator 静态配置的 URI，类似 OAuth Introspection（RFC 7662）或 OPA ExtAuthz
   - 主动发起者通过 TLS 握手即可完成服务端身份验证；无需额外签名层
2. **行业一致**：
   - OAuth Introspection（RFC 7662）：无响应签名规范，一致依赖 TLS
   - OPA External Authorization：无响应签名
   - Kong webhook-like plugins：无响应签名
   - 全部依赖 TLS + PKI
3. **TLS 正确配置下签名完全冗余**：
   - MITM / DNS 劫持在 `https://` + 合法 CA 下都被 TLS 握手阻断
   - 攻击者无法伪造目标域名证书
4. **响应签名唯一价值 = TLS 配错兜底**：
   - operator 设了 `insecure_skip_verify: true` 或用 `http://` URI 时才相关
   - 这是操作失误，不是代码缺口；正确解法是修正配置，不是给错误配置加兜底
5. **Webhook TLS 支持已完备**：`WebhookTls { enabled, certs, insecure_skip_verify }` + mTLS via `secretRef` 全套能力已存在；operator 应用
6. **URI 是 operator 静态配置**：不是用户可控输入，没有"用户诱导 Edgion 调向恶意端点"的攻击面；与 H3 ForwardAuth/Webhook SSRF NOT-AN-ISSUE 判定一致

## 判定规则

当 review / 审计报告出现以下模式时，按"业界模式一致 + 真实防线在 TLS"关闭：

1. "Webhook 响应无签名，MITM 可伪造 key 解析结果"
2. "应引入 `responseSignature` / `X-Edgion-Signature` HMAC 校验字段"
3. "应强制所有 webhook 响应必须签名"
4. "应在 resolver 内置响应签名校验常用模式"

验证三点再关闭：

- `webhook/resolver.rs::extract_value_from_response` 仍直接返回提取值，无签名校验调用
- `WebhookProviderConfig` 无 `responseSignature` / `signatureHeader` 字段
- `WebhookTls { enabled, certs, insecure_skip_verify, secret_ref }` 四件套仍完整存在

三点满足 → 架构取舍，指向 `tasks/security-audit/04-admin-api.md` L1 NOT-AN-ISSUE 决策关闭。

## 例外场景（需重评）

- OAuth Introspection / OPA ExtAuthz 等上游规范引入响应签名规范 → 评估跟随
- 出现合规要求（特定行业）强制必须签名 webhook 响应 → 按合规另立项目
- Edgion 把 webhook URI 改为可带用户输入（如 `${header.x-target}` 模板）→ 不再是 operator 静态配置，威胁模型变化，需重评

## 与其他 NOT-AN-ISSUE / 架构取舍判定的交叉引用

- `ssrf/forward-auth-webhook-ssrf.md`（H3）——同类 "URI 由 operator 静态配置" 的判定基础
- `forward-auth/default-header-forwarding.md`（M7）——同类"与 Traefik / Envoy 主流一致的设计取舍"
- `hmac-auth/replay-protection-timestamp-only.md`（M6）——同类"不加网关层新基础设施，依赖 TLS + 行业分层"

共同模式：**Edgion 不在代理层叠加冗余的、非行业标准的安全层，而是信任既有分层（TLS / PKI / RBAC / NetworkPolicy）并让 operator 正确配置**。

## 背景记录

- 决策日期：2026-04-22
- 决策来源：`tasks/security-audit/04-admin-api.md` L1（Option D — NOT-AN-ISSUE）
- 关键事实：webhook key resolver 是 "Edgion 主动调外部服务" 而非"receiver 被动收 sender 推送"，签名语义完全不同
