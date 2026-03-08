# Monitoring Metrics

This document describes how to obtain Edgion metrics and integrate with Prometheus/Grafana.

## Metrics Endpoint

The gateway process exposes an independent metrics endpoint by default (typically `:5901`). Confirm the port based on your actual deployment configuration.

## Prometheus Scrape Example

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

## Monitoring Recommendations

1. At minimum, cover request volume, error rate, and latency percentiles.
2. Differentiate by listener, route, and upstream dimensions.
3. Configure alerts for sudden spikes in 4xx/5xx responses.
4. Combine metrics with access logs for troubleshooting.

## Related Documentation

- [Access Log](./access-log.md)
- [edgion-ctl](../edgion-ctl.md)
