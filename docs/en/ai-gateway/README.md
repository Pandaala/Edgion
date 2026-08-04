# AI Gateway

> **🔌 Edgion extension**
>
> Edgion AI Gateway turns an HTTPRoute hostname into an OpenAI-compatible base URL for LLM
> traffic. It is not part of the standard Kubernetes Gateway API.

Applications call Edgion with the OpenAI Chat Completions API. Edgion owns provider endpoints,
Secret-backed credentials, model catalogs, selection, retries, usage, and cost accounting.
Clients never hold provider API keys.

## What it covers

| Topic | Description |
|-------|-------------|
| [Quick Start](./quick-start.md) | Secret → EdgionAIService → AiProxy → HTTPRoute → OpenAI SDK |
| [Concepts](./concepts.md) | Ownership split across HTTPRoute, EdgionAIService, AiProxy, and EBTP |
| [EdgionAIService](./edgion-ai-service.md) | Provider endpoint, model catalog, and typed AI backends |
| [Routing and Selection](./routing.md) | Weighted, Priority, LowestLatency, LowestUsage, CostWeighted |
| [Credentials and Limits](./credentials-and-limits.md) | Credential pools, RPM/TPM reservation, rotation, cooldowns |
| [Resilience and Failover](./resilience.md) | Credential-first retry, SSE commitment, outlier half-open, EBTP |
| [Providers](./providers.md) | OpenAI / OpenAI-compatible passthrough and Anthropic translation |
| [Quotas and Cost](./quotas-and-cost.md) | Token quota actions and estimated cost |
| [Observability](./observability.md) | Access-log `genai` ledger and optional token metrics |

## Capabilities at a glance

- **Unified OpenAI client surface** — one base URL for OpenAI, OpenAI-compatible, and Anthropic backends
- **Typed AI backends** — HTTPRoute `backendRefs` name `edgion.io/EdgionAIService` directly
- **Credential isolation** — Secret-backed pools with weighted rotation; RPM/TPM via Redis
- **Intelligent selection** — five `backendSelection` algorithms without embedding a provider list in AiProxy
- **Bounded resilience** — shared attempt ceiling, credential-first rounds, no mid-stream failover
- **Usage and cost** — durable per-attempt access-log ledger plus optional process-local Prometheus rates

## Related docs

- Plugin attachment under HTTPRoute filters: [AI Proxy (plugin stub)](../user-guide/http-route/filters/edgion-plugins/ai-proxy.md)
- Backend traffic policy for AI destinations: [Circuit Breaker](../user-guide/http-route/backends/circuit-breaker.md)
- Access log and metrics: [Access Log](../ops-guide/observability/access-log.md) · [Metrics](../ops-guide/observability/metrics.md)
