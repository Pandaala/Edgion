# HMAC Auth 防重放：时间戳窗口 + HTTPS + 业务层 idempotency 是正确分层

## 结论

`hmac_auth` 插件只做时间戳窗口（`clockSkew`，默认 300s）、**不**做 nonce 去重。这是**已评估的设计分层**，与 AWS SigV4 / Stripe Webhooks / GitHub Webhooks / Kong `hmac-auth` / APISIX `hmac-auth` 等主流 HMAC 认证实现一致。**不应**要求在网关层引入 nonce 存储（内存 LRU 或 Redis）。

遇到 review 报"HMAC Auth 应加 nonce dedup / 应引入 Redis 防重放存储"时，按下述规则关闭。

## 原因

1. **行业一致性**：
   - AWS SigV4：timestamp + signature，~15 分钟窗口，无 nonce
   - Stripe Webhooks：timestamp header，5 分钟窗口，业务层 idempotency
   - GitHub Webhooks：timestamp + signature，无 nonce
   - Kong / APISIX `hmac-auth`：时间戳窗口为唯一防线
   - OAuth 1.0a 的"必须 nonce + 服务端存储"模式已是历史方案
2. **网关层 nonce dedup 的架构代价**：
   - **内存 LRU**：多 gateway 实例独立存储，攻击者切实例即可重放，**假象防御**
   - **Redis 后端**：每请求 1 次 Redis 往返（~1-3ms 延迟）+ 引入强依赖 + Redis 故障时 fail-open vs fail-close 两难
3. **威胁模型已有分层**：
   - 中间人抓签名 → HTTPS 挡住（HMAC Auth 的基础假设）
   - 日志泄漏签名 → `hideCredentials: true` + 不写入 access log
   - 合法客户端恶意重放 → **业务层 idempotency key**（支付、改密、转账等非幂等操作的正确防线）
   - 窗口过大 → operator 缩 `clockSkew`
4. **插件已有配套机制**：
   - `clockSkew` 可调（敏感接口推荐缩到 60s）
   - `hideCredentials` 可启用（降低日志泄漏面）
   - `enforceHeaders` 可强制 `date` / `host` 等关键 header 进签名
5. **文档已明确说明**：`docs/{zh-CN,en}/user-guide/http-route/filters/edgion-plugins/hmac-auth.md` 新增"防重放安全模型" / "Replay-Protection Security Model" 小节，把分层、推荐配置、为何不做网关 dedup 的技术理由全部写清

## 判定规则

当 review / 审计报告出现以下模式时，按"设计分层"关闭：

1. "HMAC Auth 只有时间戳窗口，5 分钟内可无限重放，应加 nonce 机制"
2. "应支持 `X-Nonce` header + Redis 去重存储"
3. "应内置内存 LRU 做 nonce 记录"
4. "应参照 OAuth 1.0a 的 nonce 模型"

验证三点再关闭：

- `hmac_auth/plugin.rs` 的 `authenticate_request` 仍只做 `validate_clock_skew` + `verify_signature`（无 nonce 存储或 dedup 调用）
- `HmacAuthConfig` 无 `require_nonce` / `nonce_header` 字段
- 用户文档 "防重放安全模型" / "Replay-Protection Security Model" 小节仍在，明确声明行业分层和网关层不做 dedup 的技术理由

三点满足 → 设计分层，指向 `tasks/security-audit/01-auth-plugins.md` M6 FIXED（doc-only）决策关闭。

## 例外场景（需重评）

- 行业主流（AWS / Stripe / GitHub）转向在网关 / SDK 层强制 nonce → 重评跟随
- 真实部署反馈"业务层无法做 idempotency，只能靠网关 dedup"的具体案例出现 → 评估开独立项目加可选 Redis 后端
- 安全合规（如 PSD2 强客户认证）强制要求网关层 nonce → 按合规要求另立项目

## 与其他 opt-in / doc-only 判定的交叉引用

本决策与以下 review 规则同属"**分层解耦 + 文档引导 operator 做对应配置**"模式：

- `auth-timing/basic-auth-username-enumeration.md`（M4）——doc 里推荐 `authFailureDelayMs` + RateLimit 组合
- `oidc/nonce-pkce-defaults.md`（M8）——doc 里推荐生产启用 `useNonce` / `usePkce`
- `real-ip/max-trusted-hops-opt-in.md`（M12）——doc 里推荐双层防御（trustedIps 精确 + maxTrustedHops cap）
- 本文件（M6）——doc 里推荐 HTTPS + hideCredentials + 业务层 idempotency

共同模式：**代码保持行业一致性 + 文档明确威胁分层 + operator 按 doc 做对应部署**。

## 背景记录

- 决策日期：2026-04-22
- 决策来源：`tasks/security-audit/01-auth-plugins.md` M6（Option C — doc-only）
- 关键技术事实：网关层 nonce dedup 的多实例一致性问题是不可忽视的架构困境，单纯"加 Redis"会把这个困境推给 operator
