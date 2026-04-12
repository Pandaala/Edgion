# 注解参考

用于这些任务：

- 查找 `edgion.io/*` 注解的完整列表和语义
- 添加或修改注解时确认正确的 key 格式
- 区分当前有效注解、保留注解和仅测试注解

## 注解体系概览

Edgion 使用 `edgion.io/` 前缀的 Kubernetes 注解控制插件、路由和 Gateway 行为。注解分为以下几类：

| 类别 | 说明 |
|------|------|
| `metadata.annotations` | 直接加在资源 metadata 上的注解 |
| `options` | 通过 EdgionPlugin/EdgionTls 等 CRD 的 options 字段传入 |
| 保留键 | 已定义语义但暂未完全实现 |
| 仅测试键 | 仅用于测试，不应在生产环境使用 |

## 快速查找注解

1. **进入注解总览**：[../02-features/05-annotations/00-annotations-overview.md](../02-features/05-annotations/00-annotations-overview.md)
2. 找到对应功能模块的注解说明
3. 确认注解是否有版本变更或已废弃替代键

## 修改注解时的注意事项

- 不要向前复制遗留键（legacy key）—— 旧 key 在新版本中可能已被更名或废弃
- 修改注解语义时需要同步更新 `skills/02-features/05-annotations/` 下的文档
- 新增注解 key 需先在总览文档中注册，再在实现中添加解析逻辑
- Stream 插件的注解配置方式与 HTTP 插件不同，注意区分

## 常见踩坑

- 直接复制旧版示例 YAML，其中包含已废弃的注解 key
- 将 `options` 字段的配置与 `metadata.annotations` 混用
- 注解 value 类型错误（应为字符串，某些场景需要 JSON 转义）

## 参考文档

- 注解总览：[../02-features/05-annotations/00-annotations-overview.md](../02-features/05-annotations/00-annotations-overview.md)
