---
name: backend-tls-winner-single-snapshot
description: Historical review evidence for Concurrency; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Backend tls winner single snapshot

## Recorded conclusion

`BackendTLSPolicyState::is_winner` takes a single `timestamps.read().clone()` snapshot; TOCTOU resolved

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

## Resolution references

- `c2b-011` — —.
