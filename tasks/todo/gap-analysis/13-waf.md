# 13 - WAF (Web 应用防火墙)

**优先级**: P2 - 锦上添花

## 问题

SQL Injection、XSS、路径遍历等 OWASP Top 10 攻击的前置防御。对暴露在公网的 API 网关尤为重要。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `uri-blocker` 插件，可集成外部 WAF |
| Kong | Enterprise：Injection Protection、JSON/XML Threat Protection、Request Validator |
| Envoy | 通过 ext_proc 或 WASM filter 实现 WAF 检测 |

## 需要实现

- [ ] SQL Injection 检测（基于正则 / libinjection）
- [ ] XSS 检测
- [ ] 路径遍历检测（../、%2e%2e/ 等）
- [ ] 请求大小限制
- [ ] 可配置的规则集（类 ModSecurity CRS）
- [ ] 检测模式（block / log-only）
- [ ] 作为 HTTP 插件实现

## 备注

完整 WAF 实现复杂度很高，可考虑先做轻量版（URI blocker + 基础注入检测），或集成外部 WAF 服务。
