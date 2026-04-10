# 01 - Circuit Breaking (熔断)

**优先级**: P0 - 生产必备
**状态**: 已有前期调研，见 `../circuit-breaker/`

## 问题

Edgion 有 retry 和 timeout，但没有熔断器。当后端服务故障时无法快速切断流量，会导致级联雪崩。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `api-breaker` 插件，基于错误率的状态机（closed → open → half-open） |
| Kong | 内置于 upstream 健康检查，连续错误阈值触发熔断 |
| Envoy | 原生 circuit breaker，支持最大连接数、等待请求数、最大请求数、最大重试数四维阈值 |

## 需要实现

- [ ] 熔断器状态机（closed / open / half-open）
- [ ] 可配置的触发条件（连续错误数、错误率、慢调用率）
- [ ] 半开状态的探测策略（放行部分请求检测恢复）
- [ ] 熔断时的快速失败响应（可配置状态码和 body）
- [ ] 与 Prometheus metrics 集成（熔断状态、触发次数）
- [ ] 作为 HTTP 插件实现，支持 per-route / per-upstream 粒度

## 参考

- 已有调研：`../circuit-breaker/step-01-research.md`
- 已有实现方案：`../circuit-breaker/step-02-implementation-plan.md`
