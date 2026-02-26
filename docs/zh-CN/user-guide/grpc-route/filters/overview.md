# GRPCRoute 过滤器

GRPCRoute 可使用 Gateway API 标准过滤器和 Edgion 扩展过滤能力（按当前实现支持情况）。

## 建议策略

1. 先使用标准过滤器满足通用需求。
2. 再按需引入 Edgion 插件扩展。
3. 明确过滤器顺序，避免请求头重写和鉴权冲突。

## 相关文档

- [HTTPRoute 过滤器总览](../../http-route/filters/overview.md)
- [插件组合与引用](../../http-route/filters/plugin-composition.md)
