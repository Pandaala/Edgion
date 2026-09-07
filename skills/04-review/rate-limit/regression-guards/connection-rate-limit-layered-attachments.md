---
name: connection-rate-limit-layered-attachments
description: Use when reviewing reports that Gateway-level and route-level ConnectionRateLimit attachments both evaluate one connection.
status: accepted-tradeoff
finding_id: connection-rate-limit-double-charge
closed: 2026-08-03
knowledge_type: regression-guards
---

# ConnectionRateLimit listener and route attachments are independent policy layers — ACCEPTED

## Conclusion

Gateway-level and route-level `EdgionStreamPlugins` attachments are intentionally cumulative where
both execution stages exist. The Gateway attachment expresses a listener-wide policy, while a
TCPRoute attachment expresses a route-specific policy. A TLSRoute attachment also runs the Stage-1
runtime only through the compatibility fallback when its active Stage-2 runtime is empty. When the
two layers reference different resources, their `ConnectionRateLimit` entries have different entry
scopes and therefore independent counters. A connection must satisfy both budgets; the stricter
result is expected policy composition, not double charging.

UDP is not part of this cross-layer shape: its listener does not use Pingora's `Service` or the
Gateway-level `ConnectionFilter`, so only the UDPRoute attachment executes Stage-1 plugins.

Two `ConnectionRateLimit` entries in one resource are also independent budgets because entry scope
includes the entry index. This permits useful multi-window policies. Rejecting duplicate plugin
types would remove that capability and would not fix cross-attachment evaluation.

If the Gateway and a TCPRoute repeat the exact same resource reference, the same entry scope
evaluates twice and its counter advances twice. The same is true for a TLSRoute only when its active
Stage-2 runtime is empty and execution enters the Stage-1 compatibility fallback. This is accepted
as a low-frequency configuration overlap: operators should use separate resources for global and
route-specific budgets. Adding per-connection deduplication would require coordinating Pingora's
listener `ConnectionFilter` with later TCP/TLS route execution across several hot paths. That cost
and regression surface outweigh the benefit of making the accidental identical-reference shape
behave as one attachment.

## Review Rule

- Do not deduplicate different Gateway-level and TCP/TLS-route-level resources; they are separate
  budgets where both stages execute.
- Do not reject multiple `ConnectionRateLimit` entries merely because their plugin type matches.
- Reconsider identical-reference deduplication only after a real operator incident or when a shared
  attachment identity is already available across the listener and route execution stages.

## Separate Namespace Boundary

Whether a Gateway attachment may name an `EdgionStreamPlugins` resource in another namespace is a
separate reference-authorization question. It is tracked independently and must not be coupled to
rate-limit counter semantics.

## Resolution references

- `gw-plugin-acl-rl-04-stream` — 2026-08-03.
