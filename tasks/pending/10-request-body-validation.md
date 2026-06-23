# 10 - Request Body Validation (请求体校验)

**优先级**: P1 - 竞品标配

## 问题

在网关层拦截非法请求体，减轻后端验证压力。特别是 public API 场景，可以在流量进入后端之前拒绝格式错误的请求。Edgion 的 `request-restriction` 插件侧重于 path/header 限制，缺少 body 级别的 JSON Schema 校验。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `request-validation` 插件，支持 JSON Schema 校验 header 和 body |
| Kong | Request Validator 插件（Enterprise），支持 JSON Schema / content-type 校验 |
| Envoy | 通过 ext_proc 或 WASM 实现 |

## 需要实现

- [ ] JSON Schema 校验（Draft-07 或更高版本）
- [ ] 支持校验 request body（application/json）
- [ ] 支持校验 request header
- [ ] 可配置的校验失败响应（状态码、错误消息格式）
- [ ] Schema 可内联定义或引用外部 schema
- [ ] 作为 HTTP 插件实现

## 技术选型建议

- Edgion 已依赖 `jsonschema` v0.40 crate，可直接复用
