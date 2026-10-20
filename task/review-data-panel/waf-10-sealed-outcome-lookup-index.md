# WAF sealed outcome lookup indexing

Status: deferred sparse-index and budget decision for the user.
Source baseline: `4e8a08681`. No runtime or dependency change.

## Evidence

Gateway structured WAF inspection publishes sealed JSON/XML results before
canonical rule execution. json_inspection_runtime.rs::replay_static calls
json_inspection.rs::SealedResults::lookup_selected for a selected candidate/group.
The lookup validates its budget owner, generation identity, runtime limits,
evaluation mode and group activation, then resolves the candidate's profile.
A profile transformation error takes precedence over stored group outcomes.

After those checks, lookup_selected traverses the candidate's singly linked
outcome list. Each visited outcome charges work, resolves a generational pool
handle and verifies candidate ownership before testing its group. Repeated group
queries can revisit the same prefix; a missing group visits the whole list.
Cost depends on actual sparse outcomes and query distribution, not just compiled
rule count. profile() has its own checked traversal, so optimizing the outcome
list alone does not make the complete selected lookup constant-time.

Static replay already carries opaque sealed handles for match values and capture
keys/values into MemberMatch and TX state. The remaining capture descriptor vector
is not a second payload copy. This candidate concerns outcome search metadata,
not generic conversion of owned captures or changes to TX cloning.

## Decision

Keep the linked scan, add an independently bounded sparse lookup structure, or
reuse a previously checked lookup under an explicitly proven validity scope.
Measure outcome-list lengths, repeated group requests, no-match queries and
profile traversal separately. Preserve the sparse no-hit behavior; do not add a
dense candidate-by-group result matrix or infer no-match from an unseen profile.

An index needs real metadata admission, growth overlap and retained-byte accounting,
plus an ownership design for sealing, duplicate replacement, pool reuse and
whole-profile/candidate cleanup. Shared outcome payload aliases depend on that
cleanup boundary; an index does not authorize independent outcome deletion.
Compare construction and retained costs with saved replay work on both sparse
and many-outcome workloads. This is dynamic result-store metadata, not merely a
configuration-time matcher index.

Choose the logical work contract explicitly. Removing visited-node charges can
change near-limit acceptance. Charging a precomputed total or skipping intermediate
pool/owner checks can change sticky failure identity and order. Retain current
checked identity/profile/error semantics, or identify the precise behavioral
change for approval. Shared bytes do not make descriptor or validation work free.

## Acceptance for an approved implementation

Compare with the original ordered scan for repeated/mixed group queries, first,
last and absent outcomes, no-hit candidates, multiple profiles, late transform
errors, capture aliases and both evaluation modes. Preserve JSON/XML domain and
all owner/generation/runtime checks. Exercise whole and chunked input, duplicate
subtree replacement, discarded candidates, recycled slots and foreign/stale IDs.

Test exact and one-below work and physical limits at construction and replay,
partial admission failures, sealing and release. Compare ordered errors, usage,
selected outcomes, captures, chain rollback and later-phase reads, not only final
boolean matches. Any new optional fallback must preserve the selected contract
rather than silently changing accounting when memory is tight.

Keep the canonical bounded result owner, BodyStorage and access-log ownership.
Add no payload log, request tracing, response aggregation or integration-fixture
changes. Gateway remains the anchor; shared-leaf implementation requires a frozen
supporting-file allowlist and independent IMPL plan/final reviews. This task
records choices for the user and does not authorize implementation.

## Knowledge and verification limits

See [fused JSON inspection](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
for sparse results, shared capture handles, metered replay and cleanup ownership.
This differs from [profile capture-hit indexing](waf-09-profile-capture-hit-index.md),
which concerns compiled batch selection during inspection rather than sealed
outcome lookup during replay. No alternative index or benchmark accompanies this
task; tests of current code do not prove its potential performance benefit.
