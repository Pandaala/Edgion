---
id: ARCH-002
status: candidate
kind: architecture-debt
priority: P3
primary_partition: GW-SESSION
analyst: /root/analyze_session
independent_verification: confirmed
---

# Canonical request-body contracts are owned by the plugin-session module

## Current ownership and evidence

`request_body/mod.rs` declares capture, storage, inspection, mutation and replay
as a Gateway subsystem shared by transport, context and plugins. Its fundamental
read contracts instead live in `plugins/runtime/traits/session.rs`: `BodyReadError`
at line 36, `BodyReadResult` at 98, `RequestBodyChunkConsumer` at 103 and
`RequestBodySnapshotReader` at 119.

`request_body/{storage,state,state/enabled,operation,body_scanners}.rs` import
those contracts from plugin runtime. Storage constructs and implements the
canonical snapshot reader; the body state supplies storage-backed or proven-empty
readers. In the opposite direction, `session.rs` imports `BodyOperationError` and
`BodyScanResult` from `request_body`. This is a concrete reverse ownership dependency,
not a claim that Rust's module graph fails to compile.

## Change scenario and maintenance cost

The current WAF optimization specification adds `try_take_contiguous_memory` to
the snapshot reader and `cached_contiguous_limit` to the synchronous consumer.
The shared plugin-session file consequently declares storage-owned capability
changes. `BodySnapshotReader` implements checked resident borrowing; the adapter
consumes it under size, real-EOF and parity checks. RequestMirror also holds the
same reader independently of PluginSession after memory isolation/admission.

The methods already exist. This finding does not duplicate their implementation
task or claim performance adoption is missing. The avoidable cost is divided
ownership: body source/version/read invariants live under plugin invocation while
related operations and scanners live under the body subsystem. A storage capability
change requires following this reverse dependency to find its contract.

## Legitimate complexity and bounded improvement

Keep the trait: snapshots must own their backing, survive mutations and cross
awaits. Keep distinct admission, physical/logical limit, incomplete, conflict,
storage-failure and timeout errors; their policies differ.

Make `request_body` the canonical owner/exporter of the same four contracts.
Update actual source, tests and benchmark imports together. PluginSession retains
its stage/policy/transport methods and consumes the body-owned types. A sibling
file under plugin runtime alone does not change the ownership boundary.

The acceptance criterion is that storage, state, operations and scanners no longer
obtain fundamental body contracts from their plugin-session consumer. No extra
crate, buffer, dispatch layer, allocation, lock or compatibility re-export is
needed. A broader body-access capability would require a separate design.

## Benefit, scope and risks

This is a modest boundary improvement: one discoverable owner for body-read/source
invariants, with plugin session remaining an admission/policy adapter. Multiple
production source files mention affected contracts; that import breadth is not
permission to refactor behavior. `request_body/observer.rs` still imports PluginLog;
this proposal does not claim complete independence from every plugin-runtime type.
No incident, speedup, rebuild improvement or engineering-time measurement is claimed.

## Invariants and future verification

Preserve canonical request-only BodyStorage, immutable snapshot generations and
replay cursors, resident borrowing without joining/copying/disk reads, isolation
budgets/backing ownership, real EOF/parity/physical admission, proven-empty versus
uncaptured bodies, exact error variants/display/policy/diagnostics, prefix and stage
gates, retry/cancellation, object safety/async_trait/mockall, logging/redaction and
zero new per-chunk allocations. Existing reader/state/storage, adapter admission,
body-gate and RequestMirror tests should remain, followed by project checks during
implementation. This source-only review ran no tests or builds.

## Prior records and independent disposition

Original rules, plugin architecture/development, borrowed headers, exhaustive
dispatch, fork async boundaries, read-only body-filter design, lazy materialization,
scoped body dispatch, RequestMirror committed snapshots and
`tasks/pending/waf-optimization-design/01-spec.md` were read. None of the inspected
records assigns storage-produced contracts permanently to plugin session.

ARCH-001 concerns feature-specific operations; this is generic body-contract
ownership and can be addressed independently. Independent static review has confirmed the ownership benefit and bounded import
migration. Implementation remains future work; preserving the listed invariants
requires implementation review and the relevant tests and project checks after a
change.
