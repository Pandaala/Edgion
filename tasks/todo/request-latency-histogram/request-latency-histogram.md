# Request Latency Histogram

> 为 Gateway 添加请求延迟 Histogram 指标，支持 P50/P95/P99 分布，是 SRE 告警和 SLO 的基础。
> Priority: P0

## Background

当前 Prometheus 指标体系只有 Counter 和 Gauge，缺少请求延迟的 Histogram。
没有延迟分布，无法做：
- P99 告警规则
- SLO 定义（如 99% 请求 < 500ms）
- 精细化性能分析
- 和竞品（Kong/APISIX）的可观测性对齐

## Status

- [x] 研究阶段完成
- [x] 设计文档完成
- [ ] 实现

## 文档

| 文档 | 状态 |
|------|------|
| `step-01-research.md` | ✅ 完成 |
| 设计文档 | ✅ `docs/superpowers/specs/2026-03-22-request-latency-histogram-design.md` |

## Steps

| Step | Content | Status |
|------|---------|--------|
| step-01 | 现状分析与研究 | completed |
| step-02 | 设计文档 | completed |
| step-03 | 实现 | pending |
| step-04 | 测试与验证 | pending |
