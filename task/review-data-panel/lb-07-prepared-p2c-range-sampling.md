# Prepared P2C range sampling

Status: deferred for user decision. No runtime change or selected replacement.
Inspected baseline: `657325e18`; dependency: rand 0.9.5.

## Evidence

select_p2c receives one immutable backend slice. Empty/singleton lists return
before acquiring rand::rng. Larger lists acquire that thread-local handle once
per selection, outside the retry loop. Each pair samples 0..n; for n == 2 the
other index is 1 - i, otherwise it samples 0..n-1 and skips the excluded index.
MetricCore already owns the generation's backend slice, so n is stable for that
selector. No per-pair RNG handle acquisition or eligible-backend list exists.

The pinned Rng::random_range calls SampleRange::sample_single. The usize path
delegates through UniformUsize to integer single sampling, not construction of a
stored Uniform distribution for every draw. In the inspected Gateway default
feature graph, rand's unbiased feature is absent. Integer single sampling uses
the dependency's Canon method; stored Uniform sampling uses a precomputed range
and threshold with Lemire rejection sampling. Preparing Uniform is therefore an
algorithm substitution, not merely moving identical setup out of the hot path.
Random-word consumption, output sequences and distribution details can differ.
This is not a security or correctness finding against the existing sampler.

There is also a width boundary: exclusive usize single sampling compares high
against u32::MAX, while prepared construction first converts to inclusive high-1.
On a 64-bit target, 0..(u32::MAX+1) can therefore use different word widths between
these paths. Do not infer sequence equivalence from their common usize output.
That boundary is a dependency/API acceptance case, not a claim about fleet size.

## Decision requested

Keep current random_range, or investigate generation-owned prepared ranges for
the two bounds. Compare actual optimized code and whole-selection cost before
claiming repeated preparation is expensive or removable: inlining can eliminate
checks or hoist calculations. Prepared distributions add fields, constructor/error
handling and per-generation storage; a request-local distribution would pay its
own setup cost. No profile or benchmark currently establishes a benefit.

Preserve the existing thread-local RNG source. Do not store ThreadRng in a shared
MetricCore, create a global locked RNG, add new reseeding policy, change Cargo
randomness features, substitute modulo reduction or broaden P2C choice count.
The fixed-two design remains authoritative. Generator/reseeding/OS entropy internals
are not covered by this range-mapping inspection.

## Acceptance before implementation

Use IMPL with a frozen allow-list after a deliberate decision about changed
sampling/sequence semantics. Record rand version, unified feature graph, target
width and optimized implementation. Test range validity and distribution behavior
separately from deterministic sample identity; current tolerance tests are not a
proof of equivalent mapping or RNG consumption. Exercise empty/singleton/two-host
cases, distinct-index exclusion, zero retry budget, large bounds and 32/64-bit
width transitions without constructing impractically large backend vectors.

Preserve candidate count, metric reads before ordered health checks, equal-metric
preference, rejection behavior, final linear health-only fallback and final owned
Arc selection. Do not memoize metrics/eligibility, change degrade or snapshot
capture, or alter old-generation lifetime. Keep per-request scratch allocation-free
and avoid production-only abstractions solely for tests.

Benchmark complete selection with healthy and mostly rejected sets, varied
backend counts, all-rejected fallback and retained/rebuilt generations. Separate
RNG generation, bounded mapping, metric/health work and prepared-state storage.
No new request instrumentation, tracing, metrics or dependency is required.

## Evidence and verification limits

Sources: Gateway lb/selection/p2c.rs and lb/compiled_selector.rs; rand 0.9.5
src/rng.rs, src/distr/uniform.rs, src/distr/uniform_int.rs and the thread-local
handle accessor in src/rngs/thread.rs. Reproduce the inspected default Gateway
feature graph with `cargo tree -p edgion-gateway -e features -i rand@0.9.5`.
Feature unification in other build graphs must be checked independently.

Fresh unchanged-runtime tests passed six P2C and eight compiled-selector cases,
plus workspace all-target check/Clippy with existing warnings. No prepared-sampler
prototype, statistical equivalence study, benchmark, full suite or live transport
ran. Existing tests establish the baseline only.

Original constraints: [fixed two choices](../../skills/04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md)
and [runtime snapshot selection](../../skills/04-review/lb/regression-guards/service-runtime-snapshot-selection.md).
This is separate from [EWMA decay preparation](lb-01-precomputed-ewma-decay.md)
and [runtime eligibility lookup](backend-02-runtime-eligibility-lookup.md).
