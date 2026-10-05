---
name: fused-json-inspection
description: Review native WAF JSON dispatch, bounded retention, ordered replay and Gateway source ownership.
knowledge_type: regression-guards
---

# JSON inspection uses one grammar pass and bounded selected retention

## Conclusion and scope

The native WAF JSON path uses JsonStream and JsonChecker to validate a complete
source without a DOM or complete field-value mapping. Static predicates run as
semantic values complete. Transaction effects run later in canonical rule order.
Production edgion-waf does not depend on serde_json; test oracles may use it.
This is a JSON execution contract, not a claim that every upstream CRS rule is
supported or that arbitrary predicates visit every character only once.

## Rationale and evidence

The contract follows the authorized no-full-materialization design and the
canonical [HTTP body ownership rules](../../../01-architecture/00-common/05-unification-points.md).
Implementation ownership is split deliberately:

- json_stream.rs validates grammar and borrows contiguous unescaped tokens.
  Token decoding uses 1024 inline bytes; checker keys and paths each use 256.
  json_scratch.rs centralizes checked capacity arithmetic. Fixed inline storage
  is separate from retained heap budgets; first spill admits its full allocation.
- json_checker.rs maintains a reusable path and bounded object-key AVL index.
  Nodes and key bytes use shared contiguous arenas with per-object marks; closing
  a child truncates its live ranges while preserving parent members and reusable
  capacity. Complete-source adapters may use bounded initial capacity hints.
  Node hints are disabled unless a conservative retained-capacity bound fits;
  actual copy work remains subject to the strict cap.
  Duplicate identity is decoded key within one object instance. Replacement
  discards the preceding subtree's summary and provisional results; dotted
  external-name collisions remain separate candidates.
- json_inspection_plan.rs precompiles selectors, shared transformation profiles
  and exact named demands under compiler budgets.
- json_inspection_collector.rs and json_inspection.rs retain sparse predicate
  outcomes, captures, required name metadata and explicit deferred values.
  Same-candidate/profile/stage matches share one owning outcome's payload; aliases
  have independent captures. Generational stage tokens reject stale/cross-scope
  reuse. Whole-profile/candidate cleanup is essential: introducing independent
  outcome deletion requires a new ownership design, not reusing current aliases.
  Broad-only dispatch is preordered at compilation; mixed selectors retain their
  per-candidate selection and deduplication.
- json_inspection_runtime.rs publishes owned Bytes, reconstructs selected
  outcomes and preserves ordered chain, capture, TX and control semantics.
  Transaction selection uses a candidate cursor and bounded reusable reference
  scratch, charged to aggregate retained bytes even while taken for evaluation.
  New-unit clones omit old match observers; chain-branch clones remain complete.
  Match vectors grow only for actual hits. Cursor traversal may change competing
  physical-limit versus selector-error timing compared with eager enumeration.

Physical source/work/retention failures are immediate and sticky. Speculative
operator errors surface only when canonical selection and scheduling reach them.
Later transformation validation may override an earlier provisional predicate
hit. Duplicate replacement never erases syntax failure or spent physical work.
Final logical publication limits have explicit priority rather than an all-node
event tape preserving the deleted DOM's competing-error order.

Resource Full admission rejects unsupported JSON dependencies for both inline
and resolved bundles, including omitted JSON overrides. There is no DOM fallback.
Gateway preflights after phase-one controls, captures under the physical cap,
proves completeness, checks caps, reads once and verifies the resulting length.
Raw-body visibility retains the same immutable Bytes owner when needed.

## Alternatives and limits

Only demanded values are retained, but memory is not constant or limited to two
buffers: key identity, candidate order, profile state, matches and captures have
independent bounded storage. Dynamic broad value operands requiring every value
are rejected. Dynamic names, narrow length recipes and finite named dependencies
have separate execution paths. Raw whole-source operators may scan the original
bytes later without another JSON grammar pass.

The initial Gateway adapter still uses complete canonical BodyStorage capture.
Spilled and edited bodies can require one canonical materialization. This is not
transport-streaming inspection or a second request-body cache. Future header or
nested-JSON adapters require explicit source ownership and aggregate limits.

## Verification and re-evaluation

C1 independent review approved the replacement and legacy assertion mapping;
508 WAF library tests passed with two existing ignored cases, and API 4/4 passed.
C2 independent reruns passed resource WAF 25/25, CRD 1/1 and Gateway WAF 24/24;
strict affected-crate Clippy passed. Final verification also passed workspace
check/Clippy, 170 request-body tests and fresh WafDynamic live integration.
The configured native CRS profile passed 4,741 admitted stages, including 896
negative stages, after matching the upstream FTW XML-attribute test opt-in.
This is not full-CRS compatibility or production throughput evidence. Scoped
release measurements and their substantial remaining allocation costs are in
[the results](../../../../tasks/pending/waf-json-checker/13-results.md).

Re-evaluate when selector admissibility, duplicate ordering, transformation
validation, result lifetimes, phase controls, limit precedence or BodyStorage
materialization changes. Preserve the mirrored checker/collector/runtime tests
and Gateway completeness, one-read, source-lifetime and error-classification
regressions.

The unified follow-up design, review gates and measurements are recorded in
[the optimization spec](../../../../tasks/pending/waf-json-checker/15-optimization-spec.md).
Re-evaluate physical capacity/work thresholds when inline sizes, retained scratch,
shared outcome destruction or broad dispatch ordering changes. Preserve successful
semantic results, strict charged limits and terminal failure behavior; do not infer
identical allocator-dependent admission from unchanged logical JSON rules.

The common transformation walker also borrows unchanged ASCII-lowercase input.
The no-uppercase preflight proves identity; finding uppercase proves change and
replaces the output equality comparison. Input/output work and checkpoints remain
charged, and later transforms still validate. This does not borrow persisted match
payloads from the body or extend source lifetime. Preserve both multiMatch modes,
exact work/output bounds, repeated-transform storage and owned-wrapper behavior.

Local key-tree links use private NodeId(NonZeroUsize), with checked index-plus-one
encoding so optional links occupy one machine word. Bounds-checked node access
and scoped arena lifetime remain mandatory; these IDs are not persistent sink
receipts. Key offsets/public limits remain usize. Growth admission uses actual
Member/Frame size, including when layout changes reduce physical work or heap use.
