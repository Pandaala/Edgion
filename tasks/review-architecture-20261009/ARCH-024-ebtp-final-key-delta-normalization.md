---
id: ARCH-024
status: candidate
kind: architecture-debt
priority: P2
primary_partition: PART-256
independent_verification: confirmed
---

# Normalize EBTP deltas against previous and final key ownership

## Supported input and precise producer limitation

Gateway event_dispatch accepts add/update/delete histories. CacheData compression
classifies a final present resource as add whenever its history contains EventAdd;
compressed_events_publish_final_values_without_reclassifying_adds explicitly
covers repeated adds and delete→add becoming add. Thus an add may legally replace
a key already published to its handler. backend_traffic/store.rs treats add as
proof there is no old PreparedPolicy: its add branch patches the reverse index
with None and only update/remove branches collect prior targets into affected.

With existing P targeting A, a legal changed add P targeting B replaces the resource
map, adds B's candidate, retains A's old candidate and omits A from affected. Its
effective/background policy remains stale until a full rebuild. A changed same-
target add can retain both versions and let the old candidate keep winning.
These are source-derived supported-input cases, not executed reproductions.

Current ordinary Controller ResourceProcessor::save emits EventUpdate even for
creation, and ServerCache preserves it. This finding therefore does not assert
that current ordinary Controller delete→recreate produces EventAdd or establishes
a present normal deployment incident. It identifies an existing handler contract
failure and structural reliance on event-label semantics the owner does not promise.

## Bounded correction and alternatives

Normalize each changed key against previous prepared ownership and its final value,
using deterministic remove > update > add priority for defensive overlap. Prepare
the final resource once, collect the union of old/new targets, remove the previous
index contribution and add the final contribution once. Keep one immutable
publication and background reconciliation. Current BackendTLSPolicy's per-key
normalization supplies a local pattern, not a need for a generic framework.

Changing Gateway compression would change a separately tested contract and put
policy ownership into the wrong layer. Fixing only the add index call still omits
old targets from effective/background reconciliation. Three event-label branches
currently duplicate that previous-ownership decision and make new producer changes
risky. No controller/workqueue redesign, request state or new logging is needed.

## Invariants and future checks

Preserve no-op raw/handle retention, typed identities, invalid withdrawal, sibling
promotion, old/new union, one generation per batch, captured old snapshots and
health/outlier/degrade cleanup order. Keep the existing conf-sync write lock and
synchronous handler invocation accepted by the original locking decision.
ARCH023 is separate prepared-value storage debt.

Use isolated store and real CacheData flush wiring for P(A) delete→add P(B), same-
target changed add, invalid replacement, equivalent repeated add, defensive
overlap and full/partial final-state equivalence including background effects.
Old captured snapshots must remain valid. These checks have not been executed.

## Authority and review limits

The analyst fully read all policy primaries, actual Gateway compression and its
test, Controller producer/ServerCache sections, original atomic Service snapshot,
conf-handler and accepted partial-update-under-lock records. Controller coalescing
history is contextual, not proof of Gateway add uniqueness. No existing task owns
this bounded root. Independent verification pending.
