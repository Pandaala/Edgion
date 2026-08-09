# AI Gateway Metering and Cost

- **Status**: complete
- **Roadmap item**: AI-2
- **Depends on**: AI-0 streaming foundation and AI-1 provider/model resolution
- **Canonical roadmap**: `tasks/todo/ai-gateway/02-metering-and-cost.md`

## Goal

Add durable model-level usage and estimated-cost records plus optional Redis-backed token quota
enforcement to `AiProxy`. Streaming quota execution must batch locally, remain bounded, avoid Redis
I/O in the synchronous body hook, and reconcile to authoritative provider usage at final logging.

## Scope

- Per-model fixed-point input, output, cache-read, and cache-write prices.
- `estimated_cost_usd` in the existing structured access log.
- Optional `AiProxy.tokenQuota`; omission creates no Redis operation or quota task.
- Identity extraction through the shared `KeyGet` mechanism.
- One request-scoped batching task using N-token and T-duration triggers.
- Idempotent cumulative Redis updates, signed final correction, and fixed request-start windows.
- OpenAI-compatible quota errors and explicit default-close dependency behavior.

## Out of scope

- A virtual-key CRD or a second authentication system.
- A separate billing/audit sink; access logs are the durable ledger.
- New cost or latency metrics, model labels, or histograms.
- Hard termination of an already-committed response stream; this remains dependent on AI-4.
- Session-level aggregation and non-token billing units for audio/image endpoints.

## Progress

- [x] Architecture decision and independent audits
- [x] Price schema and access-log cost calculation
- [x] Redis cumulative quota primitive
- [x] Request-scoped N/T batching runtime
- [x] Unit verification
- [x] Real Redis integration verification
- [x] Independent code review
- [x] Local commit
