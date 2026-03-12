# 02 可观测性

> Edgion 的可观测性设计遵循三个核心原则：
> 1. **Access Log 单条还原** — 一条 access log 包含足够信息定位问题，无需翻其他日志
> 2. **只记关键节点** — 记录最终决策和异常路径，不记过程和中间状态
> 3. **避免 Metrics 爆炸** — 无 Histogram、严控 Label 基数、统一管理

## 信息分层

| 层次 | 工具 | 目标受众 | 保留时间 |
|------|------|---------|---------|
| 请求粒度 | access log（JSON） | 运维排障、审计 | 天~周 |
| 系统事件 | `tracing::info/warn/error` | 开发调试、告警 | 小时~天 |
| 性能聚合 | Prometheus metrics | SRE、Grafana | 月 |

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-access-log.md](00-access-log.md) | Access Log 字段、PluginLog 格式、常见场景速查 | ✅ 完整 |
| [01-metrics.md](01-metrics.md) | Metrics 添加步骤、Label 约束、Test Metrics 机制 | ✅ 完整 |
| [02-tracing-and-logging.md](02-tracing-and-logging.md) | 控制面日志结构化规范、安全与性能最佳实践 | ✅ 完整 |

## 提交前自检清单

- [ ] PluginLog 每条 ≤ 40 字节，以 `; ` 结尾
- [ ] 没有把调试信息写进 PluginLog（应用 `tracing::debug!`）
- [ ] 新增 metrics 通过 `GatewayMetrics` struct 管理，不直接用 `metrics::counter!()` 宏
- [ ] 新增 metrics 无 Histogram 类型
- [ ] Label 值域有限（≤ 数十个），无高基数 label
- [ ] Gauge 的所有 +1 路径都有对应的 -1
- [ ] Access log 新增字段确实有排障价值
- [ ] 没有把敏感信息写进任何日志
- [ ] tracing 日志有 `component` 字段，error/warn 有足够定位上下文
- [ ] 热路径上的 observability 操作是低开销的
