---
name: chain-verification-header-mode
description: HeaderCertAuth Header 模式证书链验证 finding 判定规则。审查"Header 模式只做单层 CA 匹配、未构建完整证书链"类 finding 时先读此文件。
---

# HeaderCertAuth Header 模式 — 单层 CA 匹配 NOT-AN-ISSUE

## 结论

`header_cert_auth` Header 模式的 `verify_against_ca()` 只做单层 CA 匹配（叶证书签名 vs 配置 CA），不构建完整 X.509 路径，判定为 **NOT-AN-ISSUE**。不接受"改用 webpki / 实现链式路径构建"类修复建议。

## 核心理由

**1. Header 模式的架构前提**

Header 模式的前置条件是存在 TLS 终止代理（Nginx / Envoy / Traefik / HAProxy）：
- 代理已在 TLS 握手层完成完整 X.509 链验证（root → intermediate → leaf 全链）
- 验证通过后才将 cert 放入 header（如 `ssl-client-cert` / `x-forwarded-client-cert`）
- Edgion 从 header 读取的 cert 已是经过代理信任的 cert

在这个架构中，`verify_against_ca()` 是**二次过滤**（"这张 cert 属于我配置的哪个 CA 组织"），不是首次安全验证。单层匹配满足此目的。

**2. 行业一致**

Kong `mtls-auth` plugin 在 header 模式下同样只做单层 CA 匹配，不在应用层重做链验证。行业模型是：链验证 = TLS 层责任；应用层读 header = 信任代理已验证。

**3. 真实攻击面在部署层**

实际安全风险是代理是否剥离外部请求里的 cert header（防止攻击者直接注入伪造 cert）。这是部署配置问题，不是 Edgion 代码问题：
- Nginx：`proxy_set_header ssl-client-cert ""` 清除外来 header 后再设置
- Envoy：自动覆盖 XFCC header，不透传外部
- 文档应说明 operator 需确认代理剥离行为

## 不接受的修复建议

- 引入 `webpki` 做路径构建（rustls 生态，与 BoringSSL 项目不兼容，且冗余）
- 用 `x509_parser` 自己实现链式遍历（代理已验证，此处冗余；实现细节多，引入新 bug 风险）
- 要求所有配置的 CA 必须通过相互验证（破坏当前 flat CA bundle 的使用方式）

## 附：Connection 模式不受影响

`mode=Connection` 路径由 Pingora/BoringSSL TLS 握手层完成完整链验证，不经过 `verify_against_ca()`，天然正确。
