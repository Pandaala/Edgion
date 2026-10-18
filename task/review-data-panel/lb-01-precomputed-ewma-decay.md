# Precomputed fractional Peak-EWMA decay

Status: deferred optimization for user decision. No runtime change.
Source baseline: bb5847108.

## Evidence

Service EWMA selection reaches CompiledSelector::select, P2C, LbBackend::score
and PeakEwmaScore::read. The upstream response-header callback also reaches the
same estimator through record_lb_latency_sample and lb::sample. The canonical
implementation is edgion-gateway/src/lb/ewma/score.rs; its design invariants are
in skills/12-algorithms/lb-peak-ewma.md.

decay_parts separates elapsed timestamp units into whole half-lives and a
remainder. HALF_LIFE_UNITS is currently 625, so the remainder has 625 possible
values. The implementation computes an integer interpolation between adjacent
entries of the 33-element DECAY_Q16 table. It returns None from 32 whole
half-lives onward. The source formula uses no floating point or heap allocation.

A compile-time table could store the exact interpolated Q16 result for each
remainder. Its logical u32 payload would be 2,500 bytes rather than the current
132 bytes; this is not a measured binary-size or resident-memory delta. The
whole-half-life calculation, remainder selection and cutoff still remain.
The larger lookup could remove fractional interpolation arithmetic, but optimized
constant divisions may already use multiplication/shifts, and table locality
can offset instruction savings. No assembly or benchmark evidence establishes
a net benefit yet.

## Decision

Choose whether to retain the small interpolated table or evaluate exact
compile-time precomputation. This is a representation tradeoff, not permission
to tune the decay algorithm, half-life, default RTT or timestamp granularity.
No per-request cache, lazy table construction or synchronization is proposed.

## Acceptance for a selected implementation

- Generate entries from the current integer interpolation formula, preserving
  every truncation step. Do not replace it with rounded floating-point powers
  or omit interpolation. Q16 unity is 65,536 and must not be narrowed to u16.
- Compare old and proposed decay_parts results exhaustively for elapsed values
  0 through 19,999, then test the 20,000 cutoff, nearby values and u32::MAX.
  Derive dimensions from the constants so later authorized changes cannot leave
  a silently stale hand-maintained table.
- Preserve read-only projection and all packed-atomic/CAS invariants. Recompute
  anchor, elapsed, decay and stored values from each newly observed CAS value;
  precomputing pure fractional factors does not authorize hoisting request state
  outside retries. Preserve monotonic anchors, neutral-directed truncation,
  saturation, zero-elapsed behavior and peak-only no-store behavior.
- Retain sustained-traffic recovery, rate-independence, below-default drift,
  stale-sample and contention regressions. Keep P2C sampling, outstanding-load
  multiplication, useful-work classification and response-header sample timing
  unchanged. Benchmark-gated policy constants remain outside this proposal.
- Inspect optimized code and benchmark score reads, successful/blended updates,
  peak-only updates and contention under representative hot/cold table access.
  Include integrated selection and response-sampling paths; do not infer a
  throughput gain from source-level division counts. Report code/data footprint
  and cache effects alongside CPU measurements.
- Keep one canonical estimator, no request-path tracing or extra metrics, and
  no runtime allocation for the table. Use a new IMPL plan with an exact
  implementation allow-list after the user selects this evaluation.

## Verification limits

The current audit passed 127 LB and 12 Service-selector network-free tests,
formatting, workspace check/Clippy and agent-document/diff validation. These
exercise the unchanged implementation; no generated-table prototype, exhaustive
old/new differential comparison, assembly inspection or performance benchmark ran.
