---
name: development
description: Development workflow skill for Edgion. Use when implementing features, adding resources, building plugins, modifying LinkSys integrations, updating handler logic, or writing internal developer documentation.
---

# 01 开发指南

> Edgion 功能开发的操作手册，涵盖资源添加、插件开发、外部连接器、配置参考等。
> 编码规范见 `../03-coding/SKILL.md`，插件相关开发优先参考本目录下的插件文档。

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-add-new-resource.md](00-add-new-resource.md) | 添加新资源类型 workflow：类型定义、处理链路、Gateway 同步、API、CRD、测试，含 4 类参考模板 | ✅ 已重构 |
| [01-edgion-plugin-dev.md](01-edgion-plugin-dev.md) | EdgionPlugin HTTP 层插件开发（阶段选择、runtime 接线、ExtensionRef、Secret 解析） | ✅ 已重构 |
| [02-stream-plugin-dev.md](02-stream-plugin-dev.md) | StreamPlugin TCP/TLS 层插件开发（ConnectionFilter / TlsRoute 两阶段） | ✅ 已重构 |
| [03-link-sys-dev.md](03-link-sys-dev.md) | LinkSys 外部系统连接器开发（Redis/Etcd/ES/Webhook，含 `LinkSysStore` 桥接） | ✅ 已重构 |
| [04-config-reference.md](04-config-reference.md) | Controller/Gateway TOML、work_dir、conf_center、EdgionGatewayConfig 分层参考 | ✅ 已重构 |
| [05-annotations-reference.md](05-annotations-reference.md) | `edgion.io/*` 注解、`options` 键、系统保留键参考 | ✅ 已重构 |
| [06-feature-flags.md](06-feature-flags.md) | Cargo Feature Flags（allocator/TLS/test 选项） | ✅ 已重构 |
| [07-documentation-writing.md](07-documentation-writing.md) | `docs/` / `skills/` / `AGENTS.md` 的文档落点、同步规则与写作 workflow | ✅ 已重构 |
| [08-conf-handler-guidelines.md](08-conf-handler-guidelines.md) | ConfHandler 开发规范：分类、增量更新、ArcSwap、配置泄漏防护 | ✅ 完整 |
| [09-status-management.md](09-status-management.md) | Status 管理规范：update_status 实现模式、已知陷阱、审查清单 | ✅ 完整 |

## 常见开发流程

### 添加新插件
1. 阅读 [01-edgion-plugin-dev.md](01-edgion-plugin-dev.md)（HTTP 插件）或 [02-stream-plugin-dev.md](02-stream-plugin-dev.md)（TCP 插件）
2. 参考 [observability/00-access-log.md](../03-coding/observability/00-access-log.md) 了解 PluginLog 规范
3. 参考 [testing/00-integration-testing.md](../04-testing/00-integration-testing.md) 添加集成测试

### 添加新资源类型
1. 阅读 [00-add-new-resource.md](00-add-new-resource.md)
2. 参考 [architecture/08-resource-system.md](../01-architecture/08-resource-system.md) 理解资源系统
3. 参考 [architecture/01-config-center/SKILL.md](../01-architecture/01-config-center/SKILL.md) 理解处理流程
4. 如涉及依赖重排队，参考 [architecture/10-requeue-mechanism.md](../01-architecture/10-requeue-mechanism.md)

### 添加外部系统连接器
1. 阅读 [03-link-sys-dev.md](03-link-sys-dev.md)
2. 参考 [testing/02-link-sys-testing.md](../04-testing/02-link-sys-testing.md) 添加集成测试

### 调整注解或 `edgion.io/*` 扩展键
1. 阅读 [05-annotations-reference.md](05-annotations-reference.md)
2. 按配置位置选择 reference：`metadata.annotations`、`options`、系统/测试保留键
3. 如涉及运行目录或配置层，补看 [04-config-reference.md](04-config-reference.md)

### 调整 Cargo Features 或构建矩阵
1. 阅读 [06-feature-flags.md](06-feature-flags.md)
2. 参考 feature matrix 判断 allocator / TLS backend 组合
3. 如涉及构建命令或 CI，再补看 [../06-cicd/SKILL.md](../06-cicd/SKILL.md)
