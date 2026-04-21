# ForwardAuth 默认全量转发请求头：设计取舍，非漏洞

## 结论

`forward_auth` 插件在 `request_headers = None` 时转发**全部**非 hop-by-hop 头（含 Cookie、Authorization）到认证服务，这是**已评估的设计取舍**，与网关行业默认一致。**不应**要求改默认值、加白名单、或强制 operator 配置 `requestHeaders`。

## 原因

1. **与网关行业默认一致**：
   - Traefik `forwardauth` — 默认转发全部
   - Envoy `ext_authz` — 默认转发全部
   - Edgion 遵循这两个主流实现的默认
   - APISIX `forward-auth` 是少数派（默认只转发列表中头），与其背离的代价大于收益
2. **URI 由 operator 静态配置**：ForwardAuth 的 `uri` 字段来自 Controller 管理的 CRD 配置，不是用户输入，不存在"用户可诱导转发到恶意地址"的场景。审计 01-auth-plugins.md 的 INFO 行明确："URI 由管理员静态配置，无用户控制 SSRF 面"
3. **文档已清晰说明**：
   - `docs/zh-CN/user-guide/edgion-plugins/forward-auth.md` 有独立小节 "### requestHeaders 行为"，列表明示 `null` / `[]` / 具体列表三种行为
   - FAQ 段对比"全量转发 vs 选择性转发"的适用场景，明确"外部鉴权服务、需要最小权限原则"推荐选择性
   - Cookie 行为有独立 callout 说明
4. **改默认的代价**：把默认改为空列表 / 白名单会破坏所有现有依赖默认行为的部署，且与 Traefik/Envoy 用户已有心智模型冲突

## 判定规则

当 review / 审计报告出现以下模式时，按"设计取舍"关闭：

1. "ForwardAuth 默认转发全部请求头（含敏感凭证）"
2. "建议默认值改为空列表 / 显式白名单，把全量模式改成显式 opt-in"
3. "文档应警告此默认行为"（文档已含独立小节说明，再加强化属噪音）
4. "运行时应 warn! 记录全量转发模式"

验证三点再关闭：

- `forward_auth/plugin.rs` 的 `build_auth_headers` 仍保留"`None` → `forward_all_headers`"分支（未默认改为白名单）
- `BasicAuthConfig`/`ForwardAuthConfig.request_headers` 仍是 `Option<Vec<String>>` 且默认 `None`
- `docs/{zh-CN,en}/user-guide/edgion-plugins/forward-auth.md` 仍含 "### requestHeaders 行为" 小节，列出三种配置的明确行为

三点满足 → 设计取舍，指向 `tasks/security-audit/01-auth-plugins.md` 的 M7 NOT-AN-ISSUE 决策，关闭。

## 例外场景（需重评）

- 如果未来把 ForwardAuth 的 `uri` 字段改为支持模板变量（如 `${request.header.x-target}`）使其变成用户可控输入 → 本结论失效，需要重新审计
- 如果行业主流默认发生变化（如 Traefik / Envoy 升级为默认白名单模式）→ 需重评是否跟随

## 背景记录

- 决策日期：2026-04-21
- 决策来源：`tasks/security-audit/` M7 条目
- 对比文件：
  - `auth-timing/basic-auth-username-enumeration.md`（M4，doc-only 关闭，有实际可利用性但改代码代价更高）
  - `auth-timing/key-auth-hashmap-not-ct.md`（M10，NOT-AN-ISSUE，无实际可利用性）
  - 本文件（M7，NOT-AN-ISSUE，有可利用性但是配置层 operator 取舍，不是代码默认疏漏）
