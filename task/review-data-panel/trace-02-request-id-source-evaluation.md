# RequestId source evaluation timing

Status: deferred optimization/API decision for user review. No runtime change.
Source baseline: ad647a342.

## Evidence

`plugins/runtime/session_adapter.rs::apply_request_ids` extracts an owned target
header and configured source before calling canonical `RequestTraceId::apply_rule`.
The core then returns Preserved for an established ID or LimitExceeded for a new
ID beyond the slot cap, discarding both inputs. An accepted incoming target also
wins without using the source. Target-invalid and suppression-marker cases must
continue to consider the source under the existing rules.

Header sources use `single_header`, preserving duplicate-value and sensitivity
rejection. Other sources use canonical `key_get`, construct an owned String and
convert it to HeaderValue. Cookie lookup scans current Cookie fields; simple
identity/path/context reads copy their strings. Query lookup additionally reaches
`ctx.cached_query()`, whose OnceLock can initialize the frozen decoded query map.
It stores an optional boxed map; absent/empty queries share the static empty map.
No measured workload establishes how often discarded extraction is significant.

There is a material timing boundary: early RequestId/preflight execution can
precede entry metadata capture. The existing query ownership knowledge notes
that reading before capture can freeze an empty cache. Skipping an otherwise
discarded early Query source could therefore affect what a later query reader
observes. Do not assume all source reads are observationally pure or silently
fix/redefine capture ordering as part of a performance patch.

The adapter applies the default patch and reconciles headers, then reconciles
again after every rule, including preserved/limited outcomes. Later rules can
read headers produced or removed by earlier rules. The trace-ID state also owns
the canonical TraceId source; moving it out temporarily to satisfy borrowing
would change what that source observes.

## Decision alternatives

- Keep eager extraction when bounded rule counts and ordinary sources make the
  work cheaper than a more complicated state transition API.
- Design a canonical decision-before-extraction operation that identifies which
  inputs are needed, then completes the same rule transition. Keep that policy
  with RequestTraceId rather than duplicating predicates in the adapter.
- Evaluate a shared borrowed-source projection independently of evaluation
  timing. Reuse existing KeyGet mechanisms; Header source cardinality/sensitivity
  cannot be replaced by a generic first-header string lookup.

Any lazy design must explicitly resolve the pre-capture query observation issue.
A closure alone does not solve Rust ownership: the adapter's source reads can
borrow the context that also contains the mutable canonical identity. Do not
introduce unsafe aliases, temporary empty identity, a second state authority or
a request-wide source snapshot. No interface or implementation is selected.

## Acceptance for later implementation

Preserve first-write value and forwarding/echo policy, suppression/non-resurrection
from stale inbound values, target/source validation and fallback order, slot and
total-byte bounds, absent markers, ApplyOutcome and aggregate capacity reporting.
Keep generation and its random calls only on the same required fallback path.
Preserve copied-value masking, including the canonical TraceId source exception.

Keep current Header duplicate/sensitivity checks, Cookie field order and invalid
field behavior, source-specific empty handling, frozen decoded Query semantics,
live header/context reads and raw versus normalized paths. Evaluate metadata
capture timing explicitly; a change needs its own reviewed requirement. Preserve
default-patch and per-rule reconciliation so later sources observe prior writes.

Compare allocations, query-map initialization, CPU and retained memory for
existing/absent/suppressed IDs, cap exhaustion, valid/invalid targets, repeated
source names and short/large query inputs. Tests should prove which source reads
are skipped and compare outcomes/headers/log identity against the current state
machine. Include real adapter rule chains, sensitive/duplicate headers, sources
that name prior destinations, and reads before/after metadata capture and rewrites.
Pure apply_rule tests alone do not verify adapter extraction timing.

Freeze an exact scope and use independent IMPL plan/final review before code
changes. No new tracing, metrics, schema knobs or broad KeyGet rewrite is implied.

## Original knowledge and verification

- [Request identity lifecycle](../../skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md)
- [Frozen query source and capture boundary](../../skills/04-review/uri-normalization/mandatory-design/normalization-contract.md#frozen-query-cache-source)

The preceding runtime batch at this baseline passed filtered runs of 29 trace-ID,
15 RequestId and 17 early-request-filter cases; these are prior results and do
not prove a lazy interface correct. This documentation audit changes no runtime
or test. No lazy prototype, new extraction-spy test or benchmark was executed.
