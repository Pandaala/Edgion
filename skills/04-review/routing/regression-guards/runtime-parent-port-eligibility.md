---
name: runtime-parent-port-eligibility
description: Review declared parent ports across route compilation and logical Gateway selection on shared listeners.
knowledge_type: regression-guards
---

# Runtime parent ports constrain logical listener selection

## Trigger and corrected behavior

Gateway A serves ports 8080 and 8081; Gateway B shares 8081. A Route declares accepted
parents A:8080 followed by B:8081. Controller projection correctly produces the physical key
union, but that union does not authorize A:8081. The previous `ResolvedParentRef` discarded
port, so the runtime could select A first on 8081 despite that undeclared attachment.

`ResolvedParentRef.port` now retains the authored optional constraint. `ListenerEntry.port`
retains the declared Gateway listener port through both index producers: the runtime store
and `GatewayInput`/`GatewayListener`/policy publisher. The shared matcher enforces port in
both its section lookup and hostname fallback. Section plus port means AND; omitted port
retains existing semantics and parent order remains unchanged.

HTTP/gRPC compilation and TCP/UDP policy-scope resolution use this shared matcher. TLS
already checks exact resolved attachments; its narrowed parent now also preserves the
authored optional port. No Controller wire field, requeue edge, per-request lookup, lock,
allocation, log or metric is added. The request path adds only an integer comparison.

## Scope and limits

This fixes logical Gateway identity and scope selection, not Route backend selection. The
review did not establish an authentication bypass. Gateway-level plugin demand is not a
separate defect in the demonstrated scenario, because A legitimately owns the Route on
8080; its demand semantics remain unchanged. The earlier
[canonical status guard](../../status/regression-guards/canonical-parent-status-ownership.md)
governs status ownership, a separate stage which cannot replace runtime port eligibility.

## Verification and re-evaluation

Fresh targeted verification on 2026-09-20:

- Gateway `parent_port`: 4 passed; shared matcher branches, omitted ports, actual HTTP/gRPC
  compilation and TCP/UDP RouteUnit parents with distinct publisher-backed class scopes.
- Gateway `runtime::` passed, including declared listener-port replacement and old-index
  preservation; `routes::`: 732 passed, including existing TLS attachment coverage.
- Plugin publication: 21 passed; stream connection bridge: 6 passed, including partial
  Gateway port movement, full restoration and immutable old publications.
- ACME challenge store: 17 passed, covering the other listener-index consumer.
- Fresh independent code and knowledge review: LGTM. An initial gRPC test-fixture Arc
  type error was corrected before these passing runs.
- Workspace check and Clippy passed; Clippy retains two unrelated existing warnings.
  Formatting, agent-document consistency, SSA force, metrics inventory, Pingora source
  policy, Gateway API transform contract and diff-whitespace checks passed.

These are in-crate tests, not a live integration run; existing `edgion-tests` files remain
unchanged. Re-evaluate when parent projection, shared physical listeners, logical listener
indexing or route/policy snapshot ownership changes.

## Resolution references

- `CAND-P2-1`: pre-existing runtime parent-port loss found during the deeper review of
  `route-parent-status-canonical-identity`; fixed separately from status ownership.
