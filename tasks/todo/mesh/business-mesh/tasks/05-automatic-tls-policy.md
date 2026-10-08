# Automatic TLS/mTLS policy reconciliation

## Meta

- Created: 2026-09-25
- Status: pending
- Type: feature
- Depends on: 04
- Branch: Not assigned
- Output: Pending

## AI guide summary

Translate enrolled business intent into existing BTP and frontend TLS resources and prove the first automated traffic path.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Create/update the agreed BackendTLSPolicy and client certificate references for backend TLS or mTLS.
- Configure existing Gateway/EdgionTls references for frontend TLS/mTLS when requested.
- Respect policy precedence, cross-namespace references, managed ownership, and forced-SSA requirements without taking over user-owned fields.
- Order credential and policy publication to avoid unintended plaintext fallback; define pending and failure states.
- Use existing dependency watches and requeues; do not add direct resource-processor calls or a second per-kind queue.

## Acceptance

- [ ] Enrollment produces working TLS and mTLS for the supported fixture without hand-editing certificate or BTP objects.
- [ ] Policy changes reach all tested replicas and negative handshake tests still reject invalid peers.
- [ ] Conflicting resources, unavailable Secrets, and denied references produce the agreed failure outcome.
- [ ] Removing enrollment does not silently disable an established authentication boundary; behavior matches task 02.

## Out of scope

Automatic Gateway Deployment provisioning, dynamic listener-port changes, and Ambient transport.

