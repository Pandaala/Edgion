---
id: ARCH-023
status: candidate
kind: architecture-debt
priority: P2
primary_partition: PART-256
independent_verification: confirmed
---

# Share prepared policy ownership across the target index

## Concrete source-derived amplification

backend_traffic/store.rs PreparedPolicy owns resource identity, complete target_keys
and precedence. The resource map holds one value; build_reverse_index and
patch_reverse_index clone the entire value once per target. Partial publication
deep-clones the resource map and reverse index. A valid policy with N distinct
Service targets therefore retains N + N² owned target keys in prepared values,
apart from the actual resource targetRefs. Unrelated partial publications clone
those lists again. Service fan-out is deliberately unbounded; the 16-target limit
applies to EdgionBackend policies, and repeated references are already deduplicated.

The reverse index needs candidate identity/precedence, not separately owned copies
of its complete attachment list. Adding another owned prepared field would
replicate it across attachments unless its author independently introduces sharing.
This is concrete ownership/allocation cost from Vec/String cloning, not a measured
heap or latency regression. The original atomic snapshot decision accepts outer
map cloning; this proposal does not claim complete publication becomes O(delta).

## Bounded alternative

Give each prepared resource one immutable owner, for example Arc<PreparedPolicy>,
retained by both the resource map and candidate lists. Keep one complete attachment
list and reuse unchanged prepared ownership when the raw resource Arc is retained.
Retain current target-local patching, exhaustive Service/AI classification and
immutable effective snapshots. Request capture still returns its existing effective
Arc; no request allocation, lock or additional lookup is needed.

A key-only index also avoids copies but adds map resolution and missing-candidate
handling to arbitration. Shared prepared ownership keeps candidate data complete.
Persistent-map migration is broader than this root. Cold Arc indirection/refcounts
are a tradeoff against owned list replication, not a new request-path mechanism.

## Invariants and future validation

Preserve raw deletion keys, canonical typed targets, unbounded Service fan-out,
idempotent duplicates, atomic malformed/mixed rejection, oldest-wins promotion,
joint resource/index/effective generations and current retry/logical-CB/AI-outlier
handle retention. Publish before background reconciliation, keep serialized conf-sync
and metadata-only logging. This is independent of ARCH024's delta correctness.

Retain full/partial equivalence, retarget, sibling, invalid and handle regressions.
Add valid multi-Service fan-out checking actual prepared/index growth and unchanged
owner retention during unrelated updates. Winners and background effects must be
identical while retained target-list storage grows linearly.

## Authority and limits

The analyst fully read nine policy primaries, validation/current dispatch, policy
tests and original ebtp-atomic-service-snapshot, ebtp-ai-target-limits-preserve-service-
fanout and conf-handler contracts. No original acceptance of nested quadratic
prepared-value replication was found. The absent historical superpowers design
pointer supplied no authority. Static review; independent verification pending.
