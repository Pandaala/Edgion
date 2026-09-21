---
name: deterministic-condition-messages
description: Condition messages must be deterministic for unchanged inputs so status echoes converge.
knowledge_type: regression-guards
---

# Deterministic condition messages

## Conclusion and scope

Gateway status messages sort conflict names before formatting them. The shared
ListenerPortManager sorts each listener's conflicting peers; GatewayHandler
sorts the aggregate names once for both ordinary port-conflict messages and
messages combined with inbound PROXY protocol conflicts.

## Rationale and evidence

Condition `message` is a meaningful part of the canonical semantic status diff.
Previously, fresh HashMap/HashSet iteration orders could change only the message,
causing PATCH, WATCH, and another reconciliation with unchanged input. Such a
sequence has no deterministic convergence bound even if a particular run happens
to repeat an ordering and stop. A resourceVersion guard does not stop a writer
that keeps deriving different messages from fresh versions.

The fix stabilizes producers, not the comparator: real diagnostic changes must
still be written. Conflict detection, ownership, and timestamp policy are unchanged.
See [status management](../../../01-architecture/01-controller/10-status-management.md).

## Verification and limits

- `ListenerPortManager::tests::conflict_messages_are_stable_across_registration_order`
  checks exact sorted messages across replacement/insertion orders and cleanup.
- `GatewayHandler` test
  `conflict_status_messages_converge_across_repeated_reconciliation` checks twelve
  successive reconciliations, both with and without a shared PROXY policy conflict.
  HTTP, TCP, and TLS listeners have distinct admission tuples but share a socket.
  Every subsequent status is semantically unchanged, including listener messages.
- These are in-process tests, not API-server admission or live WATCH tests.
  They do not establish that
  arbitrary conflicting controllers or external writers always converge.

Fresh verification on 2026-09-21: 38 Controller `conflict_` tests passed,
including both new regressions. The aggregate-reason assertion was then added
to prove both message branches execute; its targeted rerun passed. The combined
resource-processor suite passed 799 tests. Independent code/knowledge review: LGTM.

Re-evaluate when formatting a condition from unordered collections or changing
listener conflict groups, message semantics, or status comparison.

## Resolution references

- `CAND-P1-1` — unordered conflict messages sustained status-write feedback.
