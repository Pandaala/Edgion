# 01 开发指南

> Edgion 功能开发的操作手册，涵盖资源添加、插件开发、外部连接器、配置参考等。
> 编码规范见 `.cursor/rules/coding.rules`，插件开发规则见 `.cursor/rules/plugins_dev.rules`。

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-add-new-resource.md](00-add-new-resource.md) | 添加新 CRD 资源类型（10+ 文件改动清单） | TODO |
| [01-edgion-plugin-dev.md](01-edgion-plugin-dev.md) | EdgionPlugin HTTP 层插件开发（6 文件清单，含代码示例） | ✅ 完整 |
| [02-stream-plugin-dev.md](02-stream-plugin-dev.md) | StreamPlugin TCP 层插件开发（ConnectionFilter 阶段） | TODO |
| [03-link-sys-dev.md](03-link-sys-dev.md) | LinkSys 外部系统连接器开发（Redis/Etcd/ES/Kafka） | ✅ 完整 |
| [04-config-reference.md](04-config-reference.md) | Controller/Gateway TOML 配置参考 | TODO |
| [05-annotations-reference.md](05-annotations-reference.md) | `edgion.io/*` 注解参考（按作用域分类） | TODO |
| [06-feature-flags.md](06-feature-flags.md) | Cargo Feature Flags（allocator/TLS/test 选项） | TODO |
| [07-documentation-writing.md](07-documentation-writing.md) | `docs/zh-CN/` 文档编写规范与模板 | ✅ 完整 |

## 常见开发流程

### 添加新插件
1. 阅读 [01-edgion-plugin-dev.md](01-edgion-plugin-dev.md)（HTTP 插件）或 [02-stream-plugin-dev.md](02-stream-plugin-dev.md)（TCP 插件）
2. 参考 [02-observability/00-access-log.md](../02-observability/00-access-log.md) 了解 PluginLog 规范
3. 参考 [03-testing/00-integration-testing.md](../03-testing/00-integration-testing.md) 添加集成测试

### 添加新资源类型
1. 阅读 [00-add-new-resource.md](00-add-new-resource.md)（TODO: 待完善）
2. 参考 [00-architecture/08-resource-system.md](../00-architecture/08-resource-system.md) 理解资源系统
3. 参考 [00-architecture/01-config-center.md](../00-architecture/01-config-center.md) 理解处理流程

### 添加外部系统连接器
1. 阅读 [03-link-sys-dev.md](03-link-sys-dev.md)
2. 参考 [03-testing/02-link-sys-testing.md](../03-testing/02-link-sys-testing.md) 添加集成测试
