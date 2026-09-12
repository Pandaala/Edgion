# Make Partial LinkSys Provider Replacement Authoritative

## Meta
- Status: pending
- Severity: medium
- Category: correctness

## Evidence
A partial update that reuses one LinkSys key while changing its provider from Webhook to another provider publishes the new provider but does not retire the old Webhook manager/runtime entry. The stale Webhook remains callable until a later authoritative full-set update.

## Why separate
The existing Webhook lifecycle work made authoritative full-set omission and
same-provider replacement generation-safe. It did not define ownership when a
partial update reuses a key for a different provider. Fixing that gap requires
cross-provider ownership tracking and ordered cleanup semantics for every
LinkSys provider; a Webhook-only deletion would leave the shared key contract
inconsistent.

## Scope and constraints

- Trace full-set, partial add/update/remove, provider preparation, runtime-store
  publication, manager lookup, and shutdown for every LinkSys provider.
- Preserve the canonical outbound HTTP, TLS, egress, DNS, template, secret, and
  request-time lookup behavior of each provider.
- Publish the replacement before the old provider can remain callable, while
  preventing delayed cleanup from removing the new same-key generation.
- Preserve the last valid provider if replacement preparation fails; do not
  expose a missing or partially published entry.

## Acceptance criteria

- A same-key partial replacement from Webhook to any other provider, and in the
  reverse direction, leaves exactly one callable provider matching the latest
  accepted generation.
- Stale or duplicate cleanup cannot remove a newer same-key provider.
- Partial removal and authoritative full-set omission retain their existing
  behavior for every provider.
- Deterministic unit tests cover cross-provider replacement order, preparation
  failure, stale cleanup, and lookup visibility; standalone integration coverage
  verifies at least one real cross-provider transition.

## Candidate next step
Define one authoritative provider identity per LinkSys key, then implement the
transition and verification matrix above without modifying unrelated existing
integration-test files.
