---
name: coding-standards
description: Coding standards skill for Edgion. Use when reviewing or writing code that touches logging, tracing IDs, log safety, or shared project-wide implementation rules.
---

# 编码规范 — Coding Standards

> Edgion 项目通用编码规范。覆盖日志与 ID 传播、敏感信息防泄漏、控制面/数据面日志分离。
> 本规范是强制性的，新代码 **必须** 遵守，存量代码在改动时逐步对齐。

## 文件清单

| 文件 | 主题 |
|------|------|
| [00-logging-and-tracing-ids.md](00-logging-and-tracing-ids.md) | 日志 ID 传播：rv / sv / key_name 三元组，确保控制面→数据面可关联 |
| [01-log-safety.md](01-log-safety.md) | 日志安全：敏感信息不入日志、配置不泄漏、数据面禁用 tracing |

## 与其他 Skills 的关系

- **observability/** — 日志*设计*（字段定义、PluginLog 格式、Metrics 规范）
- **本目录** — 日志*编码*规范（ID 怎么传、哪些不能打印、数据面日志边界）
- **review/** — Review 时引用本规范作为判定标准
