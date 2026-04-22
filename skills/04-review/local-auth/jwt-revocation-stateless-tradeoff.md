# 本地 Admin JWT 无服务端撤销：stateless 取舍 + OIDC 作为撤销路径

## 结论

`[local_auth]` 模式签发的 admin JWT 是 **stateless** 的——`POST /api/v1/auth/logout` 只清 cookie，已签发 token 在 `exp` 前仍然有效，无服务端黑名单。这是**已评估的架构取舍**，**不应**要求在代码层引入 token 黑名单（无论是内存还是 Redis 后端）。

遇到 review 报"本地 admin JWT 应该有撤销机制 / 应该加 Redis 黑名单 / 应该改短期 access token + refresh token"时，按下述规则关闭。

## 原因

1. **行业一致性**：
   - Argo CD 本地管理员：纯 stateless JWT，无撤销，推荐 SSO
   - Kubernetes Dashboard 内置 token：同模式
   - 需要 stateful session 的产品（Portainer / Grafana / GitLab）走完全不同架构（DB-backed session，每请求 DB 查询），不是"给 JWT 加黑名单"
2. **网关层黑名单的架构困境**：
   - **内存黑名单（`HashSet<jti>`）**：多实例 HA 时每实例独立；logout 打到实例 A、后续请求打到实例 B → B 不知道撤销 → **假象防御**；进程重启清零
   - **Redis 黑名单**：Admin API 路径引入 Redis 硬依赖（当前 Redis 只是 `rate_limit_redis` 可选后端，不是 Admin API 依赖）+ 每请求 1 次 RT 延迟 + Redis 故障时 fail-open（安全失效）/ fail-close（可用性失效）两难
3. **Edgion 已内置撤销能力路径**：`unified_auth` 的 `[auth]` OIDC 配置让 IdP（Keycloak / Auth0 / Okta / Azure AD / Google）承担 session revocation——IdP 撤销 session/refresh token 即 Edgion 端立即失效。这是业界管理端认证需要撤销能力时的标准路径
4. **文档已明确 tradeoff + 提供 3 种退役运维替代操作**（`skills/02-features/02-config/03-auth-bootstrap.md`）：
   - 改密码（新登录须用新密码）
   - 轮换 `jwt_secret`（**一键 revoke all**，适合紧急情况）
   - 等窗口过期（配合缩短 `jwt_expiry_hours` 到 1-4h）
5. **CLI / Dashboard 不受 expiry 缩短影响**：`edgion-cli` `ensure_auth()` 自动 re-login；dashboard 是 UX 影响不是功能影响

## 判定规则

当 review / 审计报告出现以下模式时，按"架构取舍 + 已有 OIDC 撤销路径"关闭：

1. "logout 后 24h 内已签发 JWT 仍有效，应加服务端黑名单"
2. "应引入 Redis 后端存储 revoked jti"
3. "应改成短期 access token + refresh token 模式"
4. "应把 `jwt_expiry_hours` 默认值强制改为 1h"

验证三点再关闭：

- `local_auth/handlers.rs` 的 `logout_handler` 仍只清 cookie（无黑名单调用）
- `LocalAuthConfig` 无 `revocation_backend` / `token_blacklist` 字段
- `skills/02-features/02-config/03-auth-bootstrap.md` 仍含"JWT 撤销语义"小节，明确 stateless 取舍 + OIDC 路径 + 退役运维 3 种操作

三点满足 → 架构取舍，指向 `tasks/security-audit/03-cryptography.md` M11 FIXED（doc-only）决策关闭。

## 例外场景（需重评）

- 行业主流（Argo CD / K8s Dashboard）转向为本地 admin 提供服务端黑名单 → 重评跟随
- 出现合规要求（PCI-DSS / FIPS / 特定行业）强制管理端 token 必须可即时撤销且不能走 OIDC → 按合规要求另立项目实现
- `unified_auth` 的 OIDC 支持被删或变为可选不可靠 → 重评（届时 OIDC 撤销路径不再成立）

## 与其他 stateless/doc-only 决策的交叉引用

本决策与以下规则同属"**代码保持业界 stateless 模式 + 文档提供替代路径和配置建议**"：

- `hmac-auth/replay-protection-timestamp-only.md`（M6）——HMAC nonce dedup 同类拒绝理由
- `observability/system-error-state-metric.md`（M2）——保留紧急逃生门 + 可观测兜底
- `oidc/nonce-pkce-defaults.md`（M8）——doc-only 推荐生产 opt-in

共同模式：**代码反对引入"分布式一致性 / 强依赖"的假象防御基础设施**，把"真正需要此能力"的路径指向已有的合适机制（OIDC、业务层 idempotency、Alertmanager 等）。

## 背景记录

- 决策日期：2026-04-22
- 决策来源：`tasks/security-audit/03-cryptography.md` M11（Option E — doc-only）
- 本次改动：`skills/02-features/02-config/03-auth-bootstrap.md` 新增"JWT 撤销语义"小节 + `src/core/common/local_auth/config.rs` 的 `jwt_expiry_hours` 字段 doc comment 增强。**零代码行为变更**
- 相关参考：Argo CD 的 [Self-Signed JWT for Local Account](https://argo-cd.readthedocs.io/en/stable/operator-manual/user-management/#local-users-and-accounts) 是最接近 Edgion 本地 admin JWT 的业界对应模式
