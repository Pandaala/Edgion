---
name: resource-plugin-metadata
description: PluginMetaData 资源：插件共享配置、全局参数、被 EdgionPlugins 引用。
---

# PluginMetaData 资源

> **通用流程**: 参见 [00-resource-flow.md](00-resource-flow.md)

PluginMetaData 是 Edgion 的自定义扩展资源，用于定义插件的共享元数据配置。当多个 EdgionPlugins 实例需要共享同一套配置参数时，可以将公共部分提取为 PluginMetaData，避免重复定义。

## 源码位置

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/plugin_metadata.rs`
- 类型定义: `src/types/resources/plugin_metadata.rs`

## Controller 侧处理

### parse

无特殊处理逻辑，直接透传。PluginMetaData 是纯数据资源，Handler 不执行任何引用解析或状态管理。

无 validate、on_change、on_delete、update_status 的特殊实现。

## Gateway 侧处理

PluginMetaData 同步到 Gateway 后，存入插件配置存储，供 EdgionPlugins 引用。插件执行时读取对应的 PluginMetaData 获取共享配置参数。

## 跨资源关联

| 关联方向 | 目标资源 | 关联机制 | 说明 |
|---------|---------|---------|------|
| PluginMetaData ← EdgionPlugins | EdgionPlugins | 引用 | EdgionPlugins 引用 PluginMetaData 的共享配置 |
