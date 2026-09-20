# Unify LinkSys Runtime Ownership and Provider Retirement

## Meta
- Status: pending
- Severity: medium
- Category: correctness
- Phase: implementation and independent review complete; repository gates and live integration blocked
- Stable issue ID: link-sys-cross-provider-partial-replacement

## Evidence
A partial update that reuses one LinkSys key while changing its provider from Webhook to another provider publishes the new provider but does not retire the old Webhook manager/runtime entry. The stale Webhook remains callable until a later authoritative full-set update.

## Why separate
The existing Webhook lifecycle work made authoritative full-set omission and
same-provider replacement generation-safe. It did not define ownership when a
partial update reuses a key for a different provider. Fixing that gap requires
cross-provider ownership tracking and ordered cleanup semantics for every
LinkSys provider; a Webhook-only deletion would leave the shared key contract
inconsistent.

## Revised requirement and scope

The user accepts temporary unavailability and intermediate ambiguity during a
provider change. The required outcome is eventual convergence without another
configuration event, once the latest configuration is usable, capacity is
available, and the dependency recovers. The earlier cross-provider
last-known-good/no-missing-entry requirement is superseded, not implemented.

The requested design is one Gateway runtime table keyed by namespace/name. Each
entry is the current typed provider object, which owns its runtime lifecycle.
Live clients remain outside the serialized LinkSys CRD. All configuration
mutations notify the corresponding entry; all client lookup uses this table.

- Preserve canonical outbound HTTP, TLS, egress, DNS, templates, secrets, and
  provider-specific same-type reuse behavior.
- Remove and signal the old provider, then build the latest requested provider
  without waiting for old retirement. No deletion registry or per-key supervisor.
- Keep immutable source configuration separate where needed; it is not a second
  authority for live clients.
- Do not add CRD fields, Controller workers, a generic plugin framework, new
  failure-policy knobs, or a lossless log-delivery guarantee.

## Acceptance criteria

- A same-key replacement eventually exposes only the latest usable provider
  through fresh lookups; HttpDns/CredentialSource require no Gateway client.
- Capacity pressure and recoverable initialization failures retain pending
  intent and recover without requiring an unrelated full-set or edit.
- Owned background work has a stop path independent of the last externally held
  Arc. Deletion does not wait for every task or transport to finish.
- Stale or duplicate cleanup cannot remove a newer same-key provider.
- Partial removal and authoritative full-set omission retain their existing
  behavior for every provider.
- Deterministic no-network unit tests cover transitions, failure recovery,
  capacity, stale cleanup, and task completion. Dynamic integration tests use the
  existing shared Gateway/Controller topology, not a second standalone topology.
- Existing caller-held handles may outlive publication. The design does not
  promise immediate destruction of externally held Arcs or uninterrupted calls.
- Current runtime entries are visible to residue checks; deletion removes current
  counts. These counts do not prove all external handles or old tasks have exited.

## Plan documents

- [Design and lifecycle contract](link-sys-cross-provider-partial-replacement/01-design.md)
- [Implementation sequence and verification gates](link-sys-cross-provider-partial-replacement/03-subtasks.md)
- [Implementation and verification record](link-sys-cross-provider-partial-replacement/02-implementation.md)
- [External verification blockers](link-sys-cross-provider-partial-replacement/05-issues.md)

The user authorized the simplified plan, including obsolete lifecycle cleanup.
Keep this issue open until independent review and required checks pass; no strict
cross-provider teardown completion barrier is requested.
