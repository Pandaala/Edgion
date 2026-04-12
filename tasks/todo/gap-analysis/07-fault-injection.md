# 07 - Fault Injection (故障注入)

**优先级**: P1 - 竞品标配

## 问题

故障注入是测试系统韧性的标准手段（Chaos Engineering），可在网关层模拟后端延迟和错误，验证上游系统的容错能力。Edgion 缺少此功能。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | `fault-injection` 插件：注入延迟（delay）和错误码（abort） |
| Kong | `request-termination` 插件 |
| Envoy | 原生 fault injection filter：固定延迟 + 错误码，支持按百分比概率触发 |

## 需要实现

- [ ] 延迟注入（固定延迟 / 范围随机延迟）
- [ ] 错误注入（返回指定 HTTP 状态码和 body）
- [ ] 按百分比概率触发（如 10% 的请求注入 500ms 延迟）
- [ ] 条件触发（按 header / path 匹配特定请求）
- [ ] delay + abort 可同时生效
- [ ] 作为 HTTP 插件实现
