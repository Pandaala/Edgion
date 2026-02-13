# OpenID Connect Review Card

> 版本：当前工作分支  
> 更新时间：2026-02-12

## 威胁快照

- [x] Token 伪造（签名校验、算法白名单）
- [x] 重放（State、Nonce、会话生命周期）
- [x] Open Redirect（回调目标路径约束）
- [x] CSRF（State 校验）
- [x] Session 劫持（HttpOnly/Secure/SameSite）
- [x] 算法混淆（`none` 禁止、alg/kty/crv 约束）
- [x] Header 注入（`\r`/`\n`/`\0` 过滤）
- [x] JWKS 刷新滥用（最小刷新间隔 + singleflight）

## 验证路径

- [x] JWT：签名、`iss`/`aud`/`exp`/`nbf`
- [x] Introspection：`active` + claims 校验
- [x] State：值匹配 + 时效
- [x] Nonce：ID Token nonce 匹配
- [x] Scope：仅 `bearerOnly=true` 生效

## 密钥与会话

- [x] client/session secret 仅来自 K8s Secret（含 resolved 字段）
- [x] Session Cookie 使用 AES-256-GCM
- [x] Session Cookie 不持久化 `access_token`（内存缓存）
- [x] Cookie 大小上限 fail-fast

## 流程能力

- [x] Bearer Only（JWKS / Introspection）
- [x] Authorization Code + PKCE
- [x] Refresh singleflight
- [x] 登出清理 + 可选 revoke + end_session 跳转

## 日志与隐私

- [x] 避免记录 token/secret 明文
- [x] 安全告警日志覆盖 header 注入/超限/刷新异常

## 测试状态

- [x] OIDC 插件单测（含缓存、刷新、回退、cookie 加解密）
- [x] Secret handler 单测（OIDC secret shape）
- [x] Controller OIDC secret 解析单测
- [ ] 全链路外部 IdP 集成回归（后续扩展）
