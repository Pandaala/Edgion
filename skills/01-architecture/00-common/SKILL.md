---
name: common-conventions
description: 三个 bin（edgion-controller、edgion-gateway、edgion-ctl）共同遵守的约定：项目总览、命令行/目录/配置路径、Core 分层规范、资源系统。
---

# 00 通用约定

> 本目录描述三个 bin 共享的基础约定。在阅读任何 bin 的架构文档之前，建议先了解这些通用内容。

## 文件清单

| 文件 | 主题 | 推荐阅读场景 |
|------|------|-------------|
| [00-project-overview.md](00-project-overview.md) | 项目总览：高层架构图、Crate 结构、关键依赖 | 首次接触项目、需要全局视角 |
| [01-cli-and-startup.md](01-cli-and-startup.md) | 统一命令行约定、工作目录、配置文件路径 | 修改启动参数、理解配置加载 |
| [02-core-layout.md](02-core-layout.md) | Core 模块分层规范、放置规则 | 新增模块、避免回到旧目录结构 |
| [03-resource-system.md](03-resource-system.md) | 资源系统：define_resources! 宏、ResourceMeta、ResourceKind、Preparse | 添加新资源类型、理解资源抽象 |
