---
name: forward-auth-webhook-ssrf
description: ForwardAuth / Webhook SSRF finding 判定规则。审查"uri 字段无内网 IP 过滤，可探测元数据服务"类 finding 时先读此文件。
---

# ForwardAuth / Webhook SSRF — NOT-AN-ISSUE

## 结论

ForwardAuth `uri` 和 Webhook `uri` 缺乏内网 IP 过滤**不构成可修复的安全问题**，判定为 NOT-AN-ISSUE。不接受"加私有 IP 阻断列表"或"加 SSRF 防护库"类修复建议。

## 核心理由

**1. 代码层修复与合法用例根本冲突**

ForwardAuth 的设计目标就是调用内网认证服务。RFC 1918 私有地址（`10/8`、`172.16/12`、`192.168/16`）是 Kubernetes 集群内 auth service 的正常地址，不能阻断。`169.254.0.0/16`（云 IMDS）是可阻断的字面量 IP，但：
- 覆盖面极窄（只防直接写 IP 的低级攻击）
- 防不了 DNS rebinding 和域名形式（如 `metadata.google.internal`）
- 性价比不足以引入阻断逻辑

**2. 威胁模型要求高权限前置**

利用此路径需要：
- ForwardAuth：写 HTTPRoute 或 EdgionPlugin CRD 权限（namespace admin 级别）
- Webhook：写 LinkSys CRD 权限（集群级资源，更高权限）

拥有这些权限的攻击者已能做更大危害的事（重路由流量、注入后端、读取 Secret 等）。SSRF 不是边界突破点，而是已破坏边界内的横向移动。RBAC 是正确的防线，不是代码层 IP 过滤。

**3. 行业对照**

Traefik `forwardAuth`、Envoy `ext_authz`、APISIX `forward-auth` 均不做 URI 层 SSRF 过滤，一致依赖 RBAC + 网络策略。

## 正确的防御层

- K8s NetworkPolicy：限制 Gateway Pod 可以访问的目标 IP/Port
- RBAC：控制谁能写 HTTPRoute、LinkSys CRD
- 云侧：启用 IMDSv2（需要 PUT Token，GET 直接访问返回 401）

这些是运维/部署层控制，不是网关代码层的责任。

## 不接受的修复建议

- 加私有 CIDR 阻断列表（和合法用例冲突）
- 引入 `ssrf-guard` 或类似库（过度，且无法解决 DNS rebinding）
- 将 `169.254.x.x` 单独阻断（覆盖面太窄，且可被 DNS 绕过，性价比不足）
- 要求 `uri` 字段必须通过 DNS 解析后再校验（破坏配置校验的静态性，引入启动期 DNS 依赖）
