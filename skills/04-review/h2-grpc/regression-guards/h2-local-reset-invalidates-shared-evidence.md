---
name: h2-local-reset-invalidates-shared-evidence
description: Historical review evidence for Pingora fork / H2 END_STREAM wire observer; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 local reset invalidates shared evidence

## Recorded conclusion

Giving up wire evidence for a locally reset stream is an irreversible mark on the shared `StreamRecord`, made BEFORE the local RST_STREAM is queued and honored by publication under the same lock. Removing the pending-map entry alone is not accepted: the record is already cloned into `PeerEndStream` and the scanner cache. Evidence published strictly before the mark is deliberately NOT retracted, and readers must not consult the flag.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-local-reset-invalidates-shared-evidence.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-local-reset-invalidates-shared-evidence.md)

## Resolution references

- `H2-006` — 2026-08-27 (fixed in pingora fork `edgion_v3`).
