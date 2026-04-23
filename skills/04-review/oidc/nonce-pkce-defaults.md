# OpenidConnect `useNonce` / `usePkce` 默认关闭：设计取舍 + 文档推荐

## 结论

`openid_connect` 插件的 `use_nonce` 与 `use_pkce` 默认均为 `false`。**此为已评估的设计取舍**——OIDC 规范对授权码流定义二者为 OPTIONAL，主流网关实现一致。**不应**要求把默认改为 `true`；operator 应通过文档建议显式启用。

## 原因

1. **OIDC 规范允许**：Core spec § 3.1.2.1 对授权码流 `nonce` 定义为 OPTIONAL；PKCE（RFC 7636）是独立推荐机制，非 MUST
2. **网关行业默认一致**：
   - Kong `oidc` plugin — 默认 off
   - APISIX `openid-connect` — 默认 off
   - Envoy `oauth2` filter — 不强制
   - oauth2-proxy — 较新版本 PKCE 默认 on，nonce 仍 off
   - Edgion 与主流实现行为对齐
3. **Edgion 代码路径下主要威胁已被覆盖**：
   - 使用授权码流 + **服务器侧 token exchange**（ID Token 不经过浏览器）
   - `state` 校验（`openid_impl.rs`）挡住经典 CSRF
   - nonce / PKCE 的作用是纵深防御（IdP 压缩/重放、多客户端 token 混用、公共客户端场景等）
4. **改默认的破坏性**：
   - nonce 默认 on → 非标准 / 老旧 IdP 可能不把 `nonce` 回写到 ID Token 的 `nonce` claim → callback 校验失败 → 现有部署登录断裂
   - PKCE 默认 on → 不支持 PKCE 的 IdP（罕见但存在）会拒绝 `/authorize` 请求

## 判定规则

当 review / 审计报告出现以下模式时，按"设计取舍 + 已有文档建议"关闭：

1. "OIDC nonce 默认关闭，不防 ID Token 重放注入"
2. "use_nonce / use_pkce 默认应改为 true"
3. "应强制启用 nonce / PKCE"
4. "不启用 nonce 应在运行时 warn!"

验证三点再关闭：

- `src/types/resources/edgion_plugins/plugin_configs/openid_connect.rs` 中 `use_nonce` / `use_pkce` 默认仍为 `false`
- `src/core/gateway/plugins/http/openid_connect/openid_impl.rs` 实现分支仍正确：
  - 开启时 `/authorize` 带 `nonce` 参数、state cookie 存 `nonce`、callback 校验 `nonce` claim
  - 开启时 `/authorize` 带 `code_challenge`、token exchange 带 `code_verifier`
- `docs/{zh-CN,en}/user-guide/edgion-plugins/openid-connect.md` 仍含"**安全开关推荐配置**" / "Recommended Security Switches" 小节，明确推荐生产 `useNonce: true` + `usePkce: true` 并说明原因

三点满足 → 设计取舍已有文档建议，指向 `tasks/security-audit/01-auth-plugins.md` 的 M8 FIXED（doc-only）决策，关闭。

## 例外场景（需重评）

- 行业默认变化：若 Kong / APISIX 等同类网关把 nonce / PKCE 默认改为 on → 重评是否跟随
- OIDC 规范修订把 nonce 升级为 MUST（当前为 OPTIONAL）→ 必须改默认
- Edgion 改用 implicit flow 或 hybrid flow（spec 要求 nonce MUST）→ 改默认
- Edgion 支持"客户端提交 ID Token"模式（bypass 服务端 exchange）→ ID Token injection 立即成为直接攻击面，必须改默认

## 背景记录

- 决策日期：2026-04-21
- 决策来源：`tasks/security-audit/` M8 条目（Option B — doc-only）
- 与 M4 同 modal：有具体缓解建议 → doc-only，不改代码默认
- 与 M7 区别：M7 是 NOT-AN-ISSUE（无代码 / 无文档工作，完全关闭）；M8 有实质 operator 行动建议
