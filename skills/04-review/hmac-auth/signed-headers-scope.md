---
name: signed-headers-scope
description: HMAC Auth "签名仅覆盖已声明 header / 未签名字段可被篡改"类 finding 判定规则。先读此文件。
---

# HMAC Auth 签名仅覆盖已声明 Header — NOT-AN-ISSUE

## 结论

"HMAC Auth 未强制签名全部 header，未在 `enforce_headers` 声明的字段可被篡改而不破坏签名"——判定为 NOT-AN-ISSUE。不接受"默认强制签名全部 header"或"将 Content-Type 加入默认签名列表"类修复建议。

## 核心理由

**1. 这是 HMAC Auth 协议的设计语义**

HMAC Auth 的核心模型是：签名者显式声明哪些字段进入签名范围，验证者按同一声明验证。这与行业标准一致：

- **AWS Signature Version 4（SigV4）**：`SignedHeaders` 字段显式列出，未列入的 header 不在签名保护范围内
- **Stripe webhook**：仅签名 payload body，HTTP header 不进签名
- **GitHub webhook**：仅签名 body，header 不进签名
- **Kong `hmac-auth` plugin**：与 Edgion 同模式，operator 声明哪些 header 进签名

**2. 责任划分清晰**

未声明的 header 不在 HMAC 保护范围是**已知、显式的语义**，不是代码疏漏。Operator 若需要保护某个 header（如 `Content-Type`、`X-Custom-Action`），应将其加入 `enforce_headers` 配置。文档已说明此机制。

**3. 默认全签名的代价**

若强制签名全部 header，任何透明代理对 header 的合法修改（如添加 `Via`、调整 `Transfer-Encoding`）都会导致验签失败，严重影响可用性。

## 不接受的修复建议

- 将 `Content-Type` 或其他 header 加入默认签名列表（破坏与现有 HMAC client 的兼容性）
- 默认强制签名全部 header（代理透明修改 header 会导致验签失败）
- 警告"未签名字段可被修改"（这是设计语义，不是缺陷）

## 正确配置指引

Operator 应将所有业务关键 header 加入 `enforce_headers`：
```yaml
enforceHeaders:
  - date
  - content-type
  - x-custom-action
```
