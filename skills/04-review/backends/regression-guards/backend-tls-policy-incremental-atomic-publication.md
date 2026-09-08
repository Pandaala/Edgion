---
name: backend-tls-policy-incremental-atomic-publication
description: Use when reviewing Gateway BackendTLSPolicy partial publication, target-local arbitration, equivalent resource suppression, or snapshot atomicity.
knowledge_type: regression-guards
---

# BackendTLSPolicy partial publication is incremental and atomic

## Conclusion and scope

`BackendTLSPolicyStore` publishes the complete raw policy map and typed target
buckets in one immutable `BackendTLSPolicySnapshotData` through one `ArcSwap`.
Partial updates use structurally shared persistent maps and rebuild only targets
named by a changed policy's old or new generation. Unaffected policy entries and
`Arc<TargetBuckets>` remain shared with the prior snapshot.

Each target bucket owns the complete set of canonicalizable candidate policy keys,
including invalid candidates, together with its valid exact/whole winners and
invalid exact/whole occupancy. Removing or moving a winner therefore promotes the
next candidate by scanning only that target's candidates. It never scans unrelated
policies or rebuilds unrelated target buckets.

This is the required behavior for this resource, not a mandate that every
`ConfHandler` use the same persistent collection representation.

## Rationale and evidence

The canonical authority is the no-fake-incremental rule in
`skills/00-design-rules.md` section S and
`skills/01-architecture/02-gateway/11-conf-handler-guidelines.md` section 3.1.

Before `incremental-publication-02-backend-tls-policy`, `update` cloned the
complete `HashMap<String, Arc<BackendTLSPolicy>>`, and `publish` compared every
policy, sorted the complete policy set, and rebuilt both complete indexes. A
one-policy Watch batch therefore hid O(total policies and attachments) work
behind the partial-update interface.

The corrected snapshot has two persistent top-level maps:

- policy resource key to the complete `Arc<BackendTLSPolicy>`;
- typed `PolicyTargetKey` to `Arc<TargetBuckets>`.

For each changed resource key, publication computes its final value with the
existing defensive priority `remove > update > add`, compares the complete
resource fields, and collects canonical targets from both the old and final new
values. Candidate membership and arbitration are then rebuilt only for that
old/new target union. Full publication and partial publication share the same
single-target builder.

Complete equality includes `ObjectMeta`, every spec field, resolved Secret
contents, and status ancestors. It is deliberately field-based rather than serde
based: the admin API's thread-local `AdminRedactGuard` can replace Secret fields
with `"[redacted]"` during serialization, which is not a valid equality mechanism.
An equivalent resource reuses the old policy `Arc` and does not advance the
generation. Metadata- or status-only changes remain visible even when the
effective TLS profile is unchanged.

## Correct behavior and limits

- Lookup order remains valid exact, invalid exact occupancy, valid whole, then
  invalid whole occupancy.
- Valid arbitration remains oldest `creationTimestamp`, then resource name.
- Service and EdgionBackend targets remain isolated by `PolicyTargetKind`.
- Multi-target, missing-marker, class-mismatched, invalid-identity, and incomplete
  custom-CA policies remain invalid candidates for every canonical target they
  name.
- One compressed batch produces at most one new generation, and readers cannot
  observe policy data and target indexes from different generations.
- Capturing `BackendTLSPolicySnapshot` performs one lock-free policy-store
  snapshot load. `BackendTLSPolicySnapshot::lookup` is allocation-free against
  that captured generation. The broader Service query also reads the Service
  discovery generation and allocates its Service key; it is outside this claim.
- Full-set publication is intentionally a complete rebuild for List/reconnect.
- Target-local arbitration still scales with the number of candidates attached
  to the affected target. Structural sharing removes unrelated work; it does not
  make a single highly contended target constant-time.
- Production load/build/store publication relies on the conf-sync `CacheData`
  write lock's single-writer contract. A second writer would require CAS/RCU.

## Verification and re-evaluation

Focused Gateway unit tests cover equivalent update suppression and Arc reuse,
metadata-only publication, old/new target movement, winner removal and promotion,
multi-target single-generation batches, valid/invalid and supported/unsupported
transitions, overlapping batch priority, full/partial equivalence, typed target
isolation, exact/whole occupancy, missing markers, and allocation-free
`BackendTLSPolicySnapshot::lookup`. The allocation test does not cover the
complete `query_backend_tls_policy_for_target` Service wrapper.
The targeted `backends::policy::backend_tls` test selection passed 24 tests on
2026-09-08.

The existing `AiBackendPolicyIntegration` dynamic scenario exercises Gateway-side
policy target and TLS identity hot reload. The Controller-only BackendTLSPolicy
status ref-flap scenario is not proof of data-plane store publication.

Re-evaluate if conf-sync changes add/update/remove classification or dispatches a
second concurrent writer, target precedence changes, a new target class is added,
or request lookup stops capturing one immutable snapshot.

## Resolution references

- `incremental-publication-02-backend-tls-policy` — fixed from pending task,
  2026-09-08.
- `backends-F015` — historical complexity-comment correction; its former
  threshold implementation is superseded by this target-local publication.
- `CAND-C-1` — narrowed the allocation claim to the tested snapshot lookup API;
  the existing Service wrapper was not changed.
