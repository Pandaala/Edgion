# AI Proxy Plugin

> **🔌 Edgion extension**
>
> AI Proxy is the rule-level plugin entry for Edgion AI Gateway. Full user documentation now lives
> in the dedicated [AI Gateway](../../../../ai-gateway/) section.

## Overview

Attach exactly one enabled `AiProxy` through a rule-level `EdgionPlugins` ExtensionRef. HTTPRoute
`backendRefs` must name only `edgion.io/EdgionBackend` destinations. AiProxy owns request
translation, `backendSelection`, optional token quota, and usage/cost accounting — not the
provider list, weights, or retry attempt counts.

## Where to go next

| Topic | Document |
|-------|----------|
| End-to-end setup | [AI Gateway Quick Start](../../../../ai-gateway/quick-start.md) |
| Ownership model | [Concepts](../../../../ai-gateway/concepts.md) |
| Selection algorithms | [Routing and Selection](../../../../ai-gateway/routing.md) |
| Retries and failover | [Resilience and Failover](../../../../ai-gateway/resilience.md) |
| Providers | [Providers](../../../../ai-gateway/providers.md) |
| Quotas and cost | [Quotas and Cost](../../../../ai-gateway/quotas-and-cost.md) |
| Access log / metrics | [Observability](../../../../ai-gateway/observability.md) |

## Minimal plugin shape

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ai-proxy
  namespace: production
spec:
  requestPlugins:
    - type: AiProxy
      config:
        backendSelection:
          type: Priority
        injectUsageTracking: true
        onUnknownModel: reject
```

See the [complete configuration example](../../../../ai-gateway/quick-start.md#complete-example)
for Secret, EdgionBackend, and HTTPRoute resources.
