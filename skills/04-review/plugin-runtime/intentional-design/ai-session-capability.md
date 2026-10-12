---
name: ai-session-capability
description: Review the AI PluginSession integration boundary, lazy state and canonical batch transfer.
knowledge_type: intentional-design
---

# AI session capability

## Decision and scope

The AI portion of ARCH-001 uses one required `PluginSession::ai()` accessor
returning `&mut dyn AiSessionCapability`. Twelve operations live in
`plugins/runtime/traits/ai_session.rs`; PingoraSessionAdapter implements the trait
directly and returns itself. The implementation lives physically in
`ai/http/session.rs`, mounted only as a private child of `session_adapter.rs`;
parent fields keep their original visibility. This removes AI operation growth from the common
trait/mock without claiming whole-crate isolation: ctx() and Gateway integration
still expose feature dependencies. The separately accepted WAF decision retains
its scope and contracts.

The existing lazy AiHttpState remains the only exchange owner. Accessor and
absent-state reads/take allocate no state; canonical publication still initializes
state before checking semantic ownership, preserving its existing behavior.
There is no wrapper, new owner, copy, allocation, lock, registry or supertrait
framework. Each synchronous borrow ends before ordinary session operations or
awaits. All current feature read consumers already hold mutable session access.

## Transfer and lifecycle contracts

Client API capture precedes AiProxy rejection. Context and immutable-plan first
writers retain their resident value and consume rejected offers. Semantic
processor installation also consumes rejected offers. Canonical publication has
a distinct contract: failure returns the original offered batch unchanged and
keeps any resident batch; take consumes the resident exactly once. These
contracts must not be standardized into booleans, cloned batches or extra queues.

Guard identity deduplication/body generation, native API guard exclusion,
transcoder/duplicate/frozen conflicts, framing reservation, head-hold activation
and owner-specific pre-body fail-open disable retain their checks. Generic
processor installation, ExtProc/transcoding, response terminal arbitration,
transport and body APIs remain common. The driver still orders AI, semantic,
ordinary processors and drains canonical handoff before/after callbacks; slot
leases retain cancellation cleanup. Request BodyStorage, RawWindow and bounded
unary converter memory/process admission, deadlines and EOS remain unchanged.

Methods are required. Feature mocks configure explicit operations and attach one
MockAiSessionCapability through expect_ai().return_var(Box<dyn ...>), preserving
shared read/write identity. Manual Redis ResultSession rejects unexpected access.
No unavailable fallback instance or production overhead is added for mocks.

## Alternatives and verification limits

Keeping the old methods avoids migration cost but retains common-interface growth.
Moving only canonical records leaves other AI operations on the same boundary.
Multiple feature views or shared/mutable accessors add scaffolding without a
current shared-only consumer or independently owned state. Context-only access
bypasses transport checks; a generic runner redesign exceeds this scope. A future
canonical observation or guard-bookkeeping operation changes the feature trait,
adapter, consumers and feature mock, leaving the common operation list unchanged.
Additional capability dispatch is unmeasured; no throughput/RSS improvement claim.

Sources: traits/ai_session.rs, traits/session.rs, session_adapter.rs, ai/context.rs,
AiProxy, request/semantic guards, canonical SSE, usage extraction, streaming
Chat-to-Anthropic conversion and the response-body driver. Discriminating tests
cover real-type automock/Send short borrows, original batch pointer retention,
resident preservation/single take, allocation-free absent access, first writers,
guard generation, duplicate/frozen processor offer drops and head-owner disable.
Fresh executed check receipts and independent acceptance belong in the bounded
implementation task. Historical integration tests are not fresh refactor evidence.

Re-evaluate if adapter availability, borrowing/mock support, ownership recovery,
new shared-only callers, phase/head/terminal/cancellation or transport admission
changes. A transport-sensitive new operation legitimately still needs adapter work.

## Resolution references

- ARCH-001: AI operation-level remainder after selected WAF; bounded implementation
  evidence in tasks/pending/architecture-plugin-session-ai-capability/.
