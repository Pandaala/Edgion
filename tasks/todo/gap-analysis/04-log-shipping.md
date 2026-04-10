# 04 - Log Shipping (多目标日志投递)

**优先级**: P0 - 生产必备

## 问题

Edgion 只有本地文件日志。生产环境必须能将访问日志投递到 Kafka / Elasticsearch / Loki 等集中式日志平台，仅落盘无法满足运维需求。

## 竞品对比

| 网关 | 实现方式 |
|------|---------|
| APISIX | 15+ logger 插件：http-logger、tcp-logger、kafka-logger、elasticsearch-logger、syslog、file-logger、rocketmq-logger、clickhouse-logger、google-cloud-logging、splunk-hec-logging、loki-logger 等 |
| Kong | File Log、HTTP Log、TCP Log、UDP Log、Syslog、Kafka Log、Loggly |
| Envoy | File、stdout/stderr、gRPC Access Log Service、Fluentd、OpenTelemetry |

## 需要实现

- [ ] HTTP Logger（POST access log 到 HTTP endpoint）
- [ ] TCP Logger（发送到 TCP endpoint）
- [ ] Kafka Logger（直接写入 Kafka topic）
- [ ] 统一的 log entry 格式（复用现有 JSON access log 结构）
- [ ] 异步批量发送（buffer + flush interval）
- [ ] 失败重试和背压处理
- [ ] 可选的日志字段过滤（减少传输量）

## 优先级建议

先实现 HTTP Logger（最通用，可对接任何日志平台的 HTTP 接口），再实现 Kafka Logger（大规模场景必备）。
