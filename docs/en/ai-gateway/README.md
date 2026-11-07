# AI Gateway

> **🔌 Edgion extension**
>
> Edgion AI Gateway turns an HTTPRoute hostname into an OpenAI-compatible base URL for LLM
> traffic. It is not part of the standard Kubernetes Gateway API.

Managed entries accept OpenAI Chat Completions, native Responses creation, or native Claude Messages requests. Edgion owns provider endpoints,
Secret-backed credentials, model catalogs, selection, retries, usage, and cost accounting.
Clients never hold provider API keys.

## What it covers

| Topic | Description |
|-------|-------------|
| [Quick Start](./quick-start.md) | Secret → EdgionBackend → AiProxy → HTTPRoute → OpenAI SDK |
| [Concepts](./concepts.md) | Ownership split across HTTPRoute, EdgionBackend, AiProxy, and EBTP |
| [EdgionBackend](./edgion-backend.md) | Provider endpoint, model catalog, and typed AI backends |
| [Routing and Selection](./routing.md) | Weighted, Priority, LowestLatency, LowestUsage, CostWeighted |
| [Credentials and Limits](./credentials-and-limits.md) | Credential pools, RPM/TPM reservation, rotation, cooldowns |
| [Resilience and Failover](./resilience.md) | Failure-scoped retry, SSE commitment, outlier half-open, EBTP |
| [Providers](./providers.md) | OpenAI / OpenAI-compatible passthrough and Anthropic translation |
| [Quotas and Cost](./quotas-and-cost.md) | Token quota actions and estimated cost |
| [Observability](./observability.md) | Access-log `genai` ledger and optional token metrics |

## Capabilities at a glance

- **Unified OpenAI client surface** — one base URL for OpenAI, OpenAI-compatible, and Anthropic backends
- **Typed AI backends** — HTTPRoute `backendRefs` name `edgion.io/EdgionBackend` directly
- **Credential isolation** — Secret-backed pools with weighted rotation; RPM/TPM via Redis
- **Intelligent selection** — five `backendSelection` algorithms without embedding a provider list in AiProxy
- **Bounded resilience** — shared attempt ceiling, failure-scoped retry rounds, no mid-stream failover
- **Usage and cost** — durable per-attempt access-log ledger plus optional process-local Prometheus rates

Current support covers Chat JSON/SSE, limited JSON/SSE conversion to Anthropic, and native
Responses creation for OpenAI and native Messages for Anthropic. Gemini and Bedrock remain pending; see
[Providers](./providers.md#current-support) for the capability boundary. `api` is required: choose `openaiChat`, `openaiResponses`, or `anthropicMessages`. Native forwarding is the default; Anthropic conversion requires
`conversion: chatToAnthropic`. `usageObservation` defaults to `off`, and `injectUsageTracking`
defaults to false. Provider observation is explicit and independent of request injection.

## Related docs

- Plugin attachment under HTTPRoute filters: [AI Proxy (plugin stub)](../user-guide/http-route/filters/edgion-plugins/ai-proxy.md)
- Backend traffic policy for AI destinations: [Circuit Breaker](../user-guide/http-route/backends/circuit-breaker.md)
- Access log and metrics: [Access Log](../ops-guide/observability/access-log.md) · [Metrics](../ops-guide/observability/metrics.md)
