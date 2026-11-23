# Shed new Gateway work under process memory pressure

## Meta

| Created | Status | Type | Priority | Source |
| --- | --- | --- | --- | --- |
| 2026-11-23 | pending | feature | P2 | arch-02 |

## Scope and evidence

Track the deferred memory-pressure portion of arch-02. The Gateway can reject
new sockets at a configured connection count, and individual features bound
their own memory. The [HTTP request entry point](../../edgion-gateway/src/routes/http/proxy_http/mod.rs)
has no shared process-memory pressure decision. Memory demand can grow while
socket counts stay below the limit. The finding describes a possible OOM
consequence without measurements of an incident.

## Work and acceptance

- Define a process memory signal and budget, including behavior when the
  signal is unavailable with a supported allocator or deployment.
- Specify entry and recovery thresholds and the new work to reject under
  pressure. Start at existing Edgion admission hooks. Identify a concrete
  transport requirement before proposing a Pingora change.
- Verify transitions with deterministic pressure samples, including recovery
  and unavailable samples. Verify affected clients' admission outcomes and
  the treatment of existing exchanges, with no per-request memory sampling.

The project [declined a fixed HTTP request ceiling](../../skills/04-review/listeners/intentional-design/http-request-concurrency.md).
Preserve the existing feature budgets and live socket accounting.
