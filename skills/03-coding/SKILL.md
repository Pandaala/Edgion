---
name: coding-standards
description: Coding standards skill for Edgion. Use when reviewing or writing code that touches logging, tracing IDs, log safety, access logs, metrics, structured tracing, or shared project-wide implementation rules.
---

# 编码规范 — Coding Standards

> Edgion 项目通用编码规范。覆盖日志与 ID 传播、敏感信息防泄漏、控制面/数据面日志分离、可观测性设计（Access Log / Metrics / 控制面 Tracing）。
> 本规范是强制性的，新代码 **必须** 遵守，存量代码在改动时逐步对齐。

## 核心原则

1. **Access Log 单条还原** — 一条 access log 包含足够信息定位问题，无需翻其他日志
2. **只记关键节点** — 记录最终决策和异常路径，不记过程和中间状态
3. **避免 Metrics 爆炸** — 无 Histogram、严控 Label 基数、统一管理
4. **数据面零 tracing** — 数据面热路径禁止任何 `tracing::*` 宏

## 信息分层

| 层次 | 工具 | 目标受众 | 保留时间 |
|------|------|---------|---------|
| 请求粒度 | access log（JSON） | 运维排障、审计 | 天~周 |
| 系统事件 | `tracing::info/warn/error` | 开发调试、告警 | 小时~天 |
| 性能聚合 | Prometheus metrics | SRE、Grafana | 月 |

## 文件清单

### 日志与安全

| 文件 | 主题 |
|------|------|
| [00-logging-and-tracing-ids.md](00-logging-and-tracing-ids.md) | 日志 ID 传播：rv / sv / key_name 三元组，确保控制面→数据面可关联 |
| [01-log-safety.md](01-log-safety.md) | 日志安全：敏感信息不入日志、配置不泄漏、数据面禁用 tracing |

### 可观测性 — [observability/](observability/)

| 文件 | 主题 |
|------|------|
| [observability/00-access-log.md](observability/00-access-log.md) | Access Log 设计：字段结构、PluginLog 格式、常见场景速查 |
| [observability/01-metrics.md](observability/01-metrics.md) | Metrics 规范：添加步骤、Label 约束、Test Metrics、禁止事项 |
| [observability/02-tracing-and-logging.md](observability/02-tracing-and-logging.md) | 控制面日志：结构化 Tracing、Level 选择、错误上下文、instrument 命名 |

## 提交前自检清单

### 日志安全
- [ ] 新增的 `tracing::info!/warn!/error!` 是否在数据面热路径？如果是，改为 ctx-based
- [ ] 日志中是否包含 Secret / Token / 密钥等敏感数据？
- [ ] 日志中是否包含完整的 resource spec / backend 地址？如果是，降级到 debug
- [ ] 控制面新增日志是否包含 `kind` + `name/namespace` + `rv`？
- [ ] 如果资源已同步到数据面，access log 中是否有 `sv`？

### 可观测性
- [ ] PluginLog 每条 ≤ 40 字节，以 `; ` 结尾
- [ ] 没有把调试信息写进 PluginLog（应用 `tracing::debug!`）
- [ ] 新增 metrics 通过 `GatewayMetrics` struct 管理，不直接用 `metrics::counter!()` 宏
- [ ] 新增 metrics 无 Histogram 类型
- [ ] Label 值域有限（≤ 数十个），无高基数 label
- [ ] Gauge 的所有 +1 路径都有对应的 -1
- [ ] Access log 新增字段确实有排障价值
- [ ] tracing 日志有 `component` 字段，error/warn 有足够定位上下文
- [ ] 热路径上的 observability 操作是低开销的
- [ ] `ctx_map` 中新增的 key 是否可能包含敏感信息？如果是，需要做过滤
- [ ] 插件 stage_logs 中是否记录了完整 username / token？应只记录前缀

## 与其他 Skills 的关系

- **review/** — Review 时引用本规范作为判定标准，包含可观测性审查清单
- **01-architecture/** — 架构设计中的数据面/控制面分离决定了日志边界
