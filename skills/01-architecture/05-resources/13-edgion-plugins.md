---
name: resource-edgion-plugins
description: EdgionPlugins 资源：HTTP 插件配置、PluginRuntime 构建、条件执行、28 种内置插件。
---

# EdgionPlugins 资源

> **状态**: 框架已建立，待填充详细内容。
> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

## 待填充内容

### 功能点

<!-- TODO:
- 定义 HTTP 层插件配置
- 通过 HTTPRoute/GRPCRoute 的 ExtensionRef filter 引用
- 支持 28 种内置插件
- 支持条件执行（skip/run conditions）
- 预解析构建 PluginRuntime
-->

### Controller 侧处理

<!-- TODO:
- EdgionPluginsHandler
- 校验插件配置格式
- 可能引用 Secret（认证类插件）
-->

### Gateway 侧处理

<!-- TODO:
- 在路由 preparse 时构建 PluginRuntime
- 非每请求创建
-->

### 跨资源关联

<!-- TODO:
- ← HTTPRoute/GRPCRoute: 通过 ExtensionRef 引用
- → Secret: 认证凭证
- → PluginMetaData: 插件元数据
- → ReferenceGrant: 跨命名空间引用
-->
