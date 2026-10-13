---
name: backend-tls-winner-single-snapshot
description: Target-local policy timestamp projection uses one read guard, separate from reference lookup.
knowledge_type: regression-guards
---

# Backend tls winner single snapshot

## Current contract

`BackendTLSPolicyState::is_winner` projects only the selected target's policy keys
and copied timestamps under one timestamp read guard, then releases the guard
before sorting. This preserves a coherent view of timestamp values without
cloning unrelated entries (`ARCH-099`). Separate per-entry `get_timestamp` calls
would permit writes between reads and must not replace the guarded projection.

The target-reference lookup happens separately; this is not an atomic snapshot
spanning both owners. The zero/one-reference shortcut, oldest timestamp first,
present timestamps before missing values, and namespace/name tie-break remain
unchanged. `set_timestamp(None)` removes a previous timestamp. Both policy
handlers reuse the implementation with separate runtime state instances.

Focused tests cover unrelated timestamps, removal and missing-value precedence;
existing tests cover target keys, ordering, section isolation and sibling requeues.
Source inspection establishes the removal of full-map copying; no benchmark or
production latency improvement is claimed.

## Evidence boundary

The earlier `c2b-011` correction cloned the full timestamp map once instead of
locking each lookup independently. Its recorded "TOCTOU resolved" conclusion
applies only to timestamp-read coherence, not to the separately owned reference
map. Retaining unrelated owned entries was an implementation detail. Re-evaluate
when either owner's locking, arbitration comparator or handler registration changes.

## Resolution references

- `c2b-011` — —.
