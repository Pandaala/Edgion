# 监控指标

本文档说明如何获取 Edgion 指标并接入 Prometheus/Grafana。

## 指标端点

网关进程默认暴露独立指标端点（通常是 `:5901`）。请按实际部署配置确认端口。

## Prometheus 抓取示例

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: edgion-gateway
  namespace: monitoring
spec:
  selector:
    matchLabels:
      app: edgion-gateway
  namespaceSelector:
    matchNames:
      - gateway-system
  endpoints:
    - port: metrics
      interval: 15s
      path: /metrics
```

## 监控建议

1. 至少覆盖请求量、错误率、延迟分位数。
2. 区分 listener、route、upstream 维度。
3. 对突增的 4xx/5xx 配置告警。
4. 将指标与 access log 结合排障。

## 相关文档

- [访问日志](./access-log.md)
- [edgion-ctl](../edgion-ctl.md)
