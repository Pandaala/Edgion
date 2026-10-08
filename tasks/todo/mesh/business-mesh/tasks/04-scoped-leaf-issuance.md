# Scoped leaf issuance and delivery

## Meta

- Created: 2026-09-25
- Status: pending
- Type: feature
- Depends on: 03
- Branch: Not assigned
- Output: Pending

## AI guide summary

Issue authorized server/client certificates and deliver them using the agreed existing Secret references.

Read the [task index](../03-subtasks.md) and [overall specification](../01-spec.md)
before starting. Follow the shared execution rules in the index.

## Scope

- Derive permitted identities from enrolled resources and the task 02 contract; constrain namespaces, SANs, usages, and validity.
- Issue backend server and Gateway client certificates; cover frontend certificates when configured.
- Write matching certificate/key pairs atomically and reconcile only explicitly owned Secrets.
- Connect the supported backend fixture to certificate delivery and reload; expose no claim of automatically configuring arbitrary applications.
- Use the canonical secret-redaction and schema/transport rules for any new resolved fields.

## Acceptance

- [ ] Issued certificates verify against the business CA with the correct SANs and intended server/client usages.
- [ ] Out-of-scope resources and unauthorized identities cannot obtain credentials.
- [ ] Repeated reconciliation avoids unnecessary reissuance and preserves user-owned Secrets.
- [ ] A receiving backend and Gateway can load the issued credentials; private keys are absent from diagnostics.

## Out of scope

Workload attestation for node proxies, transparent interception, and arbitrary application configuration mutation.

