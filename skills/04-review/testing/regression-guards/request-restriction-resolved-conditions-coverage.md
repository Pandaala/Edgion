---
name: request-restriction-resolved-conditions-coverage
description: Historical review evidence for Integration / RequestRestriction resolved conditions; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Request restriction resolved conditions coverage

## Recorded conclusion

Closed with current-tree execution evidence: after an environment-only disk guard was resolved with an explicitly authorized `cargo clean`, the full fast integration script rebuilt every binary and passed. The RequestRestriction suite covered present `ipRefs`, entry-level `run`, resolved HMAC credentials, and the cl-01 unresolved-reference fail-close cases; the overall run also ended with zero unknown fields, no requeue dead loop, no cache residue, and clean service shutdown.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `cl-11` — 2026-08-24.

ARCH-028 B2 keeps ordinary IP references live: required Deny and entry/body roots
must be preflighted together before a matching sibling, outer skip or stage-specific
shortcut. Required absence returns a recoverable generic 500; Allow alternatives
remain incomplete candidates. Controller status and generation clearing tests verify
read proofs, not copied CIDR payloads. Dynamic ConditionRefs phases update/delete/
restore only the IpList while retaining ordinary and RAP plugin resources.
