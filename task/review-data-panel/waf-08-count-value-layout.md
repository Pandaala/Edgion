# WAF count-value layout

Status: deferred representation and physical-budget decision for the user.
Source baseline: `df7e1d072`. No runtime or dependency change.

## Evidence

Gateway WAF target selection in transaction.rs::select_values already counts
matching candidates into a scalar and emits one SelectedValue. Exclusions compact
that vector in place; count evaluation does not build a separate payload list.

SelectedBytes::Count stores digits as [u8; 40] plus a start offset. SelectedValue
contains this enum and is Copy, so every selection slot inherits the resulting
enum layout, even when its active variant is a request range or structured handle.
The unsigned decimal_bytes helper formats usize; on 64-bit targets its maximum
requires 20 decimal digits. This establishes excess digit capacity, not an exact
SelectedValue size reduction: alignment, discriminants and other variants matter.
No layout measurement or workload benchmark has been performed here.

The representation is coupled to physical admission. SelectionStorage::reserve
and summary_capacity use size_of::<SelectedValue>() for capacity, growth overlap
and retained-byte checks. Transaction scratch accounting uses the same size.
A smaller slot may change near-limit admission and summary-preflight feasibility;
it cannot be treated as a byte-for-byte accounting-neutral local edit.

The shared unsigned helper also feeds numeric capture keys, response status,
request metadata and JSON/XML publication. JsonPublication and XmlPublication
retain two [u8; 40] metadata buffers. The subsequent projection-only optimization
computes numeric length directly with checked_ilog10, so request-head preflight
no longer depends on the formatter width. Changing the formatter alone still
conflicts with fixed consumer types. signed_decimal_bytes is a separate i64
formatter and is not automatically part of this candidate.

## Decision

Keep the current representation, use a narrower count-only representation, or
change the canonical unsigned buffer width across its consumers. Any chosen width
must follow the supported integer-width contract and preserve zero and maximum
values; avoid scattered magic sizes or a duplicate decimal conversion algorithm.
A count-only design must justify its conversion/storage costs and borrowed output
lifetime. A shared change must explicitly inventory and update every consumer.

Measure actual sizes and alignment on supported targets, then compare selection
vector capacity and peak overlap separately from logical selected-value counts.
Decide the intended changed physical-budget boundary rather than preserving an
obsolete sizeof charge or silently claiming unchanged admission. Compare short
and long selection lists, count-heavy and ordinary targets, reused scratch and
near-budget fallback paths before claiming throughput or memory improvement.

## Acceptance for an approved implementation

Verify zero, powers of ten and adjacent values, usize::MAX and offset conversion.
Preserve emitted bytes, selection order, target counts and selector/control work.
Keep signed minima and unrelated signed formatting unchanged unless separately
included in the reviewed scope. No integer truncation or new payload allocation
is an acceptable shortcut.

Reconcile request-head projection with actual arena construction, all numeric
capture keys, JSON/XML metadata slices and their retained representations. Verify
exact and one-below selection/retained limits, geometric growth, conservative
reallocation overlap, reused capacity and summary fallback. Ordinary, multiMatch
and fused JSON paths must agree on their applicable accounting rules; physical
layout measurements do not replace semantic error-order tests.

Keep one canonical formatter and bounded selection owner. Add no second body
cache, request tracing, payload logging or metrics. The Gateway remains the anchor;
future shared-leaf implementation requires an exact supporting-file allowlist and
IMPL plan/final reviews. Keep integration fixtures unchanged. This task records
choices for the user and does not authorize implementation.

## Knowledge and verification limits

See [fused JSON inspection](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
for reusable selection scratch and summary preflight boundaries. Current tests
verify current behavior; they do not measure an alternative layout or approve
changed admission. No runtime change or benchmark accompanies this task.
