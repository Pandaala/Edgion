---
name: resource-http-route
description: HTTPRoute 资源：路由规则、匹配条件、BackendRef、Filter、插件绑定、路由注册。
---

# HTTPRoute 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 定义 HTTP 路由规则（hostnames, rules, matches, filters, backendRefs）
- 支持 Path (Exact/Prefix/RegularExpression)
- 支持 Method/Headers/QueryParams 匹配
- 支持 RequestHeaderModifier/ResponseHeaderModifier/URLRewrite/RequestRedirect/RequestMirror/ExtensionRef filter
- 通过 ExtensionRef 绑定 EdgionPlugins
- parentRefs 挂载到 Gateway
-->

### Controller 侧处理

<!-- TODO:
- HttpRouteHandler:
  - 校验 parentRefs（Gateway 是否存在、端口是否匹配）
  - 校验 backendRefs（Service 是否存在、跨命名空间引用）
  - 校验 filter 引用（EdgionPlugins、LoadBalancer 等）
  - 预解析 PluginRuntime
  - 更新 gateway_route_index
  - 更新 AttachedRouteTracker
  - route_utils 共享校验逻辑
-->

### Gateway 侧处理

<!-- TODO:
- ConfHandler 更新路由表
- 按 port bucket → 构建 DomainRouteRules → ArcSwap 原子切换
- 构建 PluginRuntime（预解析阶段，非每请求）
- 路由匹配详见 02-gateway/03-routes/01-http-route.md
-->

### 跨资源关联

<!-- TODO:
- → Gateway: parentRefs 挂载
- → Service: backendRefs 后端引用
- → EdgionPlugins: ExtensionRef filter
- → PluginMetaData: 插件元数据
- → ReferenceGrant: 跨命名空间引用需要授权
- ← Secret: 如果 filter 需要引用 Secret
-->
