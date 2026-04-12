# 11 - GraphQL Support (GraphQL 支持)

**优先级**: P2 - 锦上添花

## 问题

随着 GraphQL 普及，网关层的 GraphQL 感知能力越来越重要。可以实现按 operation name 路由、防止深度查询攻击、GraphQL 专用限流等。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | 按 GraphQL operation / field 属性进行路由匹配 |
| Kong | GraphQL 专用限流、缓存、Proxy Caching Advanced、DeGraphQL（REST → GraphQL） |
| Envoy | 无原生支持（通过 WASM 或 ext_proc 实现） |

## 需要实现

- [ ] GraphQL 请求解析（query / mutation / subscription 识别）
- [ ] 按 operation name 路由
- [ ] 查询深度限制（防止深度嵌套攻击）
- [ ] 查询复杂度限制
- [ ] GraphQL 专用限流（按 operation 粒度）
- [ ] 作为 HTTP 插件实现
