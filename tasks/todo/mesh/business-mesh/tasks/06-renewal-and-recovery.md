# Automatic renewal and recovery

## Meta

- Created: 2026-09-25
- Status: pending
- Type: feature
- Depends on: 05
- Branch: Not assigned
- Output: Pending

## AI guide summary

Complete the automatic certificate lifecycle and prove recovery while retaining existing traffic protections.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Schedule leaf renewal ahead of expiry using the agreed lifecycle mechanism, bounded retries, and canonical time handling.
- Persist and recover enough state for restart and failover; avoid duplicate issuance and repeated no-op writes.
- Publish updated certificate/key pairs through existing Secret sync and verify backend reload independently.
- Handle renewal failure and expiry without weakening TLS validation or downgrading transport.
- Define a controlled CA rollover procedure with overlapping trust and convergence checks; automatic root rotation is optional and requires a separate decision.

## Acceptance

- [ ] A shortened-lifetime fixture renews automatically and new handshakes on every tested replica use the replacement.
- [ ] Controller restart, replica failover, temporary API failure, and signing failure recover without changing the CA unexpectedly.
- [ ] HTTP/1 pooled connections and HTTP/2 streams demonstrate documented rotation semantics; existing connections are not falsely reported as revoked.
- [ ] Old trust is retired only after the tested rollover procedure; health probes remain correct.

## Out of scope

Immediate revocation of established sessions and mandatory automatic root rotation.
