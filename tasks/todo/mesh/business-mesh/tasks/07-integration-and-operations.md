# Integration acceptance and operations guide

## Meta

- Created: 2026-09-25
- Status: pending
- Type: feature
- Depends on: 06
- Branch: Not assigned
- Output: Pending

## AI guide summary

Validate the complete business Mesh flow and document the supported deployment and operational contract.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Assemble the earlier fixtures into a repeatable integration scenario using repository test conventions.
- Exercise multiple Services, fixed namespace boundaries, multiple Gateway replicas, and Controller restart/failover.
- Cover issuance, policy updates, renewal, negative identities, conflicts, safe removal, and recovery as one lifecycle.
- Document required RBAC, CA persistence/backup, backend reload requirements, entry naming, certificate rotation, and failure diagnosis.
- Record frontend versus backend protection and direct-access enforcement; document safe disable/migration when authentication depends on the Gateway.

## Acceptance

- [ ] Recorded fresh integration results demonstrate the automatic lifecycle without cert-manager or manual leaf renewal.
- [ ] User-facing examples and the overall spec agree with actual supported configuration and limitations.
- [ ] No private signing key is delivered to serving Gateways; unrelated namespaces and user-owned resources remain unchanged.
- [ ] Relevant compile/lint/schema/doc guards and targeted tests pass; remaining limitations are explicitly recorded.

## Out of scope

Unrequested broad refactoring, full-suite execution without user request, and claiming transparent workload Mesh support.
