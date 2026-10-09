# Persistent business CA

## Meta

- Created: 2026-09-25
- Status: pending
- Type: feature
- Depends on: 02
- Branch: Not assigned
- Output: Pending

## AI guide summary

Bootstrap and recover a business signing authority without cert-manager or CA replacement on ordinary restart.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Implement the agreed CA storage and bootstrap/import contract, reusing existing signing utilities where applicable.
- Coordinate Controller replicas and use atomic creation or the chosen ownership mechanism to prevent competing CA generations.
- Validate stored CA material, restrict signing access, and keep the signing key out of Gateway configuration sync and admin output.
- Define missing, invalid, or unexpectedly deleted CA behavior; surface a recoverable failure instead of silently replacing an established trust domain.

## Acceptance

- [ ] Fresh initialization creates one usable CA; restart preserves its identity.
- [ ] Concurrent initialization and failover converge on the same persisted CA.
- [ ] Invalid material and simulated persistence failures do not publish a new trust domain or leak private material.
- [ ] Focused tests cover bootstrap, reload, concurrency, and failure behavior.

## Out of scope

Leaf renewal scheduling, automatic root rotation, and public ACME issuance.
