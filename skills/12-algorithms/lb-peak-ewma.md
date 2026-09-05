---
name: lb-peak-ewma
description: Latency scoring for the Ewma load-balancer policy — the lock-free time-decayed Peak-EWMA estimator in lb/ewma/score.rs, the four defects of the fixed-alpha formula it replaced (three correctness-grade), the Tower/Linkerd comparison that drove each divergence, and the six implementation invariants plus the two constants still awaiting benchmark.
status: shipped (constants benchmark-gated)
---

# Latency EWMA scoring — load balancing

> **Landed 2026-09-04** in `edgion-gateway/src/lb/ewma/score.rs`. The mechanism ships; the two
> constants (`DEFAULT_RTT_US`, `HALF_LIFE_UNITS`) are still benchmark-gated placeholders, so the
> "Replacement criteria" section below is the acceptance gate that has **not** yet been run.
> Sections describing the old fixed-alpha estimator are retained as the record of what was
> replaced and why.

The `Ewma` LB policy scores each backend by a latency estimate and picks the lower of two
P2C-sampled candidates. This entry covers the estimate itself, not the sampler
(see [../04-review/lb/p2c-choice-count-fixed-at-two.md](../04-review/lb/p2c-choice-count-fixed-at-two.md) for P2C).

## What we run today

`edgion-gateway/src/lb/ewma/score.rs` — `PeakEwmaScore`, one packed `AtomicU64`
per backend, held by `EdgionBackend::score`:

```text
[63:32] ts_units : u32   monotonic, 16 ms granularity
[31:0]  rtt_us   : u32   saturating
read(now)  = default + (stored - default) * 2^(-elapsed/H)     # pure projection
observe(s) = if s >= read(now) { s } else { eff*d + s*(1-d) }  # peak-immediate
score(now) = read(now).saturating_mul(conn + 1)
```

Shipped constants, both **benchmark-gated placeholders**:

| Constant | Value | Note |
|---|---|---|
| `DEFAULT_RTT_US` | `30_000` (30 ms) | Linkerd's `default_rtt`. Replaces the old 1 ms seed, which under peak semantics plus the `conn + 1` multiplier made a fresh backend look 30x better than a healthy one. |
| `HALF_LIFE_UNITS` | `625` (10 s at 16 ms) | Linkerd's `decay`, corroborated by Kong's `DECAY_TIME`. **Shared** with the overload penalty's bleed-off rate, so it cannot be tuned for latency alone. |

Sample point is response headers (`pg_upstream_response_filter.rs`), taken on every response
that produces headers — a retried attempt keeps its sample. A **server error is
recorded peak-only** via `observe_peak_only`, never through the blend: an error's
elapsed time is not a measurement of useful work, and a backend failing fast
would otherwise acquire the lowest score and pull traffic into a black hole.

The global `alpha` knob is gone; the blend weight is the elapsed-derived decay
factor, so smoothing follows each backend's own request rate rather than a
process-wide setting.

## What we ran before 2026-09-04 (replaced)

Per backend, one `AtomicU64` holding microseconds, initialized to
`INITIAL_EWMA_US = 1_000` (1 ms), in `edgion-gateway/src/lb/edgion_backend.rs`.

```text
update(sample):  ewma' = (alpha * sample + (100 - alpha) * ewma) / 100
read():          ewma
```

- `alpha` was a process-global `AtomicU32` percent value, default 20 (= 0.20).
  **Deleted** when the replacement landed — it had no production call site.
- The blend widened to `u128` intermediates so an extreme sample could not overflow
  (see [../04-review/lb/update-ewma-u128-intermediate.md](../04-review/lb/update-ewma-u128-intermediate.md)).
- `update` ran only on `FinalizeMode::Settle`; a retried attempt used `Discard` and
  contributed no sample. Both the modes and the type are gone.
- `Settle` fired from `lb_runtime_cleanup` at request end.
- The sample was `Instant::now() - upstream.start_time` — the **whole upstream attempt**.

## Contract of the replaced estimator

| Property | Value |
|---|---|
| Read cost | one `AtomicU64::load(Relaxed)` |
| Write cost | CAS loop, once per settled request |
| Concurrency | lock-free; no allocation |
| Time input | none — the estimate had no time dimension |
| Determinism | fully deterministic; its tests serialized on a process-global alpha guard |

The shipped estimator keeps the lock-free, allocation-free read/write costs, adds
a monotonic time input, and needs no test-wide lock because there is no global
parameter left to serialize on — tests inject `now` directly.

## Defects of the replaced estimator

Four, of which the first two and the fourth are correctness-grade. Each is closed
by the shipped replacement; they are retained as the record of why it exists.

### 1. Absorbing state — a spiked backend is starved permanently

The estimate moves **only** when the backend is selected and its request settles. A backend
that records one slow sample loses every subsequent P2C matchup against healthy peers, so it
receives no traffic, so it produces no new sample, so the estimate never falls. For `n == 2`
the `1 - i` fast path in `edgion-gateway/src/lb/selection/p2c.rs` makes the pairing
deterministic and the starvation absolute.

It does not self-heal across config changes: `replace_backends` reuses the same `Arc` per
address (`edgion-gateway/src/lb/compiled_selector.rs`), and the test
`replace_backends_carries_over_metrics_for_existing_addr` asserts the EWMA survives. Active
health checks do not help — the backend is healthy, just permanently outranked. Only a process
restart or the endpoint address disappearing clears it.

**Operational symptom**: a backend's traffic drops to zero after one latency blip and never
returns, with green health checks and no log or metric indicating why. Effective service
capacity silently becomes N-1.

### 2. The sample is stream duration, not latency

`Settle` runs after the response body has been fully proxied, so the sample includes response
streaming time. For SSE and large downloads the "latency EWMA" is really an average stream
duration, which then feeds defect 1: a backend serving one long stream is scored in the tens
of seconds and enters the absorbing state. On an AI gateway this is the common case, not an
edge case.

### 3. Decay rate tracks QPS, not time

With no time term, how fast history is forgotten depends on request rate. A high-QPS backend
forgets within seconds; a low-QPS one retains a sample indefinitely. There is also no
outstanding-load term, so a burst piles onto the historically-fastest backend until samples
return.

Adjacent finding: `set_alpha` had no production call site anywhere in the workspace — alpha was
permanently 20 and the setter was dead configuration surface. Deleted with the estimator.

### 4. Slow failures are invisible, so the estimator is asymmetric

A retried attempt was finalized with `FinalizeMode::Discard`, which dropped the EWMA sample.
There are **two** explicit call sites, both in
`edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`: `begin_upstream_attempt`
(l.1210) and its AI pre-connect analogue `begin_ai_peer_attempt` (l.1373). A third path is
`LbCommitGuard`'s `Drop` (`lb/attempt.rs`), which discards any guard left unresolved.
All three had to be handled when the sample point moved.

So an attempt that answers slowly and is then **retried away from** contributes nothing to its
backend's score: attempt 1 takes 8 s to return 503, is retried, records no sample; attempt 2
succeeds elsewhere and only that backend is scored.

Combined with defect 1 this is a one-way ratchet *within the retried population*. A backend
that is slow and succeeds is penalized, permanently. A backend that is slow and fails **into a
retry** keeps a pristine score and keeps winning P2C matchups.

Scope precisely: this bites only when a retry actually happens — the response status matches
`retry.codes`, the AI retry decision fires, or the failure is at connect. A slow 503 with no
matching retry policy is `Settle`d at request end and does record its (body-inclusive) sample.
An attempt that fails at connect records nothing either way, because `commit_attempt` runs only
from `pg_connected_to_upstream.rs:45`.

This was deliberate — the comment in `edgion-gateway/src/lb/attempt.rs` reads "retry must not
pollute prior backend" — and it is defensible while the sample means *whole-attempt duration*,
because a retried attempt's duration is arguably not that backend's fault. It stops being
defensible once the sample means *time to response headers*, which is a self-contained
measurement of what that backend did. See the sample-point discussion below.

## Prior art — Tower / Linkerd (verified against source)

> **Corrected 2026-09-04.** An earlier revision of this entry said linkerd2-proxy has no
> estimator of its own and merely wraps Tower. That is no longer true, and the difference
> matters: linkerd now ships a first-party `linkerd/ewma` crate whose read path is **pure**.
> The Tower analysis below is retained because it is still accurate *about Tower*, which
> `linkerd/proxy/balance` still wraps for the plain (non-penalized) `NewBalance` path — but the
> conclusion "lazy decay cannot be a pure read" was a property of Tower's storage, not of the
> algorithm. The penalized `NewPenaltyPeakEwmaBalance` path uses the first-party
> `linkerd/ewma` crate instead. See
> [Linkerd's own crate](#linkerds-own-ewma-crate-verified-at-a66af811) below.

### Tower `PeakEwma`

`tower::load::peak_ewma::PeakEwma` state is:

```rust
struct RttEstimate {
    update_at: Instant,
    rtt_ns: f64,
}
```

Decay is expressed as "feed a zero-latency sample", reusing the update path:

```rust
fn decay(&mut self, decay_ns: f64) -> f64 {
    let now = Instant::now();
    self.update(now, now, decay_ns)
}
```

The update is asymmetric — this is what "Peak" means:

```rust
self.rtt_ns = if self.rtt_ns < rtt {
    // For Peak-EWMA, always use the worst-case (peak) value as the
    // estimate for subsequent requests.
    rtt
} else {
    let elapsed = nanos(now.saturating_duration_since(self.update_at));
    let decay = (-elapsed / decay_ns).exp();
    let recency = 1.0 - decay;
    (self.rtt_ns * decay) + (rtt * recency)
};
```

Load multiplies by outstanding requests, counted for free via the `Arc` refcount that each
in-flight `Handle` holds:

```rust
fn load(&self) -> Self::Metric {
    let pending = Arc::strong_count(&self.rtt_estimate) as u32 - 1;
    let estimate = self.update_estimate();
    Cost(estimate * f64::from(pending + 1))
}
```

Two further facts that matter for us:

- State is `Arc<Mutex<RttEstimate>>`. **Tower's read path takes the lock and writes back**,
  because `update_at` must advance or the same elapsed span would decay repeatedly. In Tower,
  lazy decay is not a pure read. This is a storage choice, not an algorithmic necessity —
  see below.
- `PeakEwma<S, C = CompleteOnResponse>` — the default completion policy stops the RTT clock at
  the **response headers**, excluding body streaming.
- Parameters are per-backend from the control plane (`Backend.BalanceP2C.PeakEwma { decay,
  default_rtt }`), typically `decay: 10s`, `default_rtt: 30ms`.

### Linkerd's own EWMA crate (verified at `a66af811`)

`linkerd/ewma/src/lib.rs` is a first-party estimator, not a Tower re-export. It backs the
**penalized** balancer only; the plain balancer still wraps Tower. Storage is
`{ value: f64, decay: f64, timestamp: Instant }`, held by `load-biaser`'s `SharedState::rtt`
as `RwLock<Ewma>`. `MIN_DECAY = 1ms`
guards against division by zero.

The read is a **pure projection** — `&self`, no write-back:

```rust
pub fn get_at(&self, now: time::Instant) -> f64 {
    if self.value.is_infinite() || now <= self.timestamp {
        return self.value;
    }
    let elapsed = now.saturating_duration_since(self.timestamp);
    self.value * (-elapsed.as_secs_f64() / self.decay).exp()
}
```

The write derives alpha from elapsed time, which is where the fixed-alpha knob goes away:

```rust
let alpha = 1.0 - (-elapsed.as_secs_f64() / self.decay).exp();
self.value * (1.0 - alpha) + value * alpha
```

and `add_peak` is the asymmetric branch — a sample above the decayed projection *replaces*
the value and resets the decay reference point, rather than blending.

Two consequences for our proposal. First, a pure lazy-decay read is demonstrably achievable,
so our packed-`AtomicU64` design is not fighting the algorithm — it is only removing the
remaining `RwLock`, which we need because `select_p2c` reads two backends per selection.
Second, `get_at` decays toward **0**, same as Tower, so our decay-toward-`default_rtt`
divergence stands against linkerd too and needs its own justification (it has one, below).

## The replacement, as shipped

Adopt the Tower/linkerd semantics; reject both storage choices. Tower's `Arc<Mutex<_>>` is
viable there because each endpoint is a separate service, and linkerd's `RwLock<Ewma>` is
viable because its `get_at` is a pure read taken once per `load()`. Our `select_p2c` touches
two backends per selection on the request path, so any lock is disqualifying.

### Score representation — one packed `AtomicU64`

Lazy decay needs to read value and timestamp as a consistent pair. Packing avoids both a lock
and a torn read across two atomics:

```text
[63:32] ts_units : u32   monotonic, 16 ms granularity (~2.2 years before wrap)
[31:0]  rtt_us   : u32   saturating (~71 minutes)
```

One `load(Relaxed)` to read, one `compare_exchange_weak` to write. `EdgionBackend` keeps the
same field count — the field becomes `score: PeakEwmaScore`, same size. At a 10 s half-life the
16 ms quantization error is ≈0.11%.

### Decay — toward `default_rtt`, fixed-point

```text
read(now) = default + (stored - default) * d,    d = 2^(-elapsed / half_life)
```

`d` comes from a 32-entry Q16 table plus a shift by whole half-lives, short-circuiting to
`default` past ~32 half-lives. No `f64`, no libm on the request path: one table index, one
multiply, two shifts.

**Deliberate divergence**: Tower and linkerd both decay toward 0, so a long-idle backend scores best and wins
every matchup until its first sample lands; it relies on the pending multiplier to brake. We
decay toward `default_rtt` so that *no information is a neutral score, not a winning one*, and
a spiked backend relaxes to exactly the same score as a never-used one. Recovery becomes
directly assertable in tests.

### Update — peak-immediate, alpha deleted

```text
eff    = read(now)
stored = if sample > eff { sample } else { eff * d + sample * (1 - d) }
ts     = now
```

The smoothing weight **is** the decay factor, exactly as in Tower and in linkerd's `Ewma::add`.

**Consequence surfaced during implementation (2026-09-04):** at zero elapsed the sample weight
is `1 − d = 0`, so a *faster* sample arriving in the same timestamp tick is discarded entirely.
Linkerd has the identical property — its `add` early-returns on `ts <= self.timestamp` — but
hides it behind nanosecond timestamps. Our 16 ms quantization makes it bite a whole tick's worth
of samples: at high QPS only peaks register, and recovery is carried by decay toward
`default_rtt` rather than by fast samples. Since decay-toward-default *is* the designed recovery
path, this is accepted. It must not be "fixed" by flooring the sample weight, which would
reintroduce precisely the QPS-dependent smoothing this design removes. Asserted by
`faster_sample_at_zero_elapsed_is_ignored` in `edgion-gateway/src/lb/ewma/score.rs`.

This removes `alpha`
entirely: a high-QPS backend (small elapsed, `d ≈ 1`) smooths heavily, a long-idle one
(`d ≈ 0`) adopts the new sample outright — which is defect 3's fix, with no knob. Since
`set_alpha` is already dead code, this is a net parameter removal, not an addition.

### Sample point — response headers

The sample is taken in
`edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs`, matching Tower's
`CompleteOnResponse`, rather than at request end in `pg_logging::lb_runtime_cleanup`. This was
the only way to fix defect 2.

**Resolved and shipped 2026-09-04 — record unconditionally.** A deliberate behavior change,
signed off before implementation. The replaced estimator discarded a retried attempt's sample.
Recording at header time produces the sample before the retry decision is known, so the choice
must be made explicitly. It is recorded. `FinalizeMode` was deleted outright as a consequence — with the sample moved
off the guard, `Settle` and `Discard` no longer differ, so `LbCommitGuard` now covers
outstanding-connection accounting only. Four reasons:

1. **The measurement becomes self-contained.** At header time the sample is exactly "this
   backend's time to first byte on this attempt". Nothing that happens afterwards changes
   whether that is true, so there is nothing for a later retry decision to invalidate. The old
   rationale ("retry must not pollute prior backend") was sound only while the sample meant
   whole-attempt duration.
2. **It closes defect 4.** Discarding on retry is precisely what makes slow failures
   invisible while slow successes are punished.
3. **The overload penalty requires it.** A 429 is frequently itself the retry trigger. If
   `Discard` still suppressed EWMA writes, the penalty write and the latency write on the same
   response would have contradictory semantics — see
   [lb-overload-penalty.md](lb-overload-penalty.md).
4. **It matches the prior art.** Tower's `CompleteOnResponse` has no retry awareness at all;
   in linkerd the balancer sits *below* the retry layer, so every attempt is an independent
   service call and every attempt records.

A connection error or timeout that never produces response headers still records no sample.
That is correct — no measurement exists — and outlier detection owns that failure.

### Implementation invariants

These are correctness requirements, not style notes. Each has a failure mode that is silent.

1. **Anchor on the truncated timestamp.** Store `floor(now / 16ms)` and compute elapsed from
   the stored truncated value. Truncation errors then telescope —
   `Σ(floor(t_i) − floor(t_{i−1})) = floor(t_n) − floor(t_0)` — so total drift is bounded by one
   quantization unit regardless of write count. Storing true-`now` while computing elapsed from
   a truncated anchor compounds the error per write, and a high-QPS write storm will pin the
   score. This is what makes repeated penalty writes idempotent in
   [lb-overload-penalty.md](lb-overload-penalty.md).
2. **Recompute inside the CAS loop.** Both the decayed read and the value to be stored must be
   derived from the value observed on each retry, never hoisted above the loop — otherwise a
   concurrent higher peak is stomped. `PeakEwmaScore::observe` is the reference shape.
3. **Widen the Q16 intermediate, and compute on the magnitude.** `|stored − default| * 65536`
   reaches ~2.8e14 and needs a 64-bit intermediate. Compute on the unsigned magnitude and
   reapply the sign afterwards: `>>` on a *negative* i64 floors toward negative infinity, which
   grows the magnitude and biases a below-default backend **away** from neutral, inflating a
   fast backend's advantage. Decay-from-below is a first-class case here precisely because we
   decay toward `default_rtt` rather than toward 0. Same class of bug as
   [../04-review/lb/update-ewma-u128-intermediate.md](../04-review/lb/update-ewma-u128-intermediate.md).
4. **Saturate at both boundaries.** The store-side conversion to u32 µs must saturate, never
   `as u32`; the outstanding-load multiply must be `saturating_mul`.
5. **Truncating fixed-point can freeze values near parity.** When `|v − default| * (1 − d) < 1`
   µs the value stops moving. At a 16 ms step `1 − d ≈ 0.00111`, so the window is within
   ~900 µs of `default`. That is at near-parity and harmless. Do **not** "fix" it with
   round-half-up, which converts a benign freeze into a slow upward leak.
6. **Interpolate the fractional decay weight; never truncate to the table index.** One index
   spans `HALF_LIFE_UNITS / FRAC_STEPS = 19.53` units = **312.5 ms**, twenty times the 16 ms
   timestamp granularity. Truncating makes `d` exactly 1.0 for every elapsed under 320 ms —
   zero decay *and* zero weight for a new sample — and because `observe` re-anchors the
   timestamp on every write, a backend receiving traffic more often than every 320 ms then
   never decays at all. Measured on the un-interpolated first implementation: a 500 ms spike
   followed by 120 s of 5 ms samples left the score at 500 000 **unchanged** at every rate down
   to 10 rps, recovering only below ~3 rps. That is defect 1's absorbing state, sustained by
   traffic instead of by starvation. It shipped and was caught in review; the regressions
   `spiked_backend_recovers_under_sustained_traffic` and `decay_is_rate_independent` exist
   because the original suite tested only idle recovery — the one case that worked.

   Residual per-write value drift after interpolation is at most one µs per *decaying* write,
   always toward `default_rtt` (the safe direction), and it does not compound on reads
   because `read` never stores. Invariant 1's telescoping bound covers **timestamps only** and
   does not bound value drift; the two are separate claims.

### Outstanding-load multiplier

```text
score = read(now).saturating_mul(conn + 1)
```

We do not need Tower's refcount trick — `EdgionBackend` already has `conn_count`. Both scoring
policies now maintain it (`lb/attempt.rs`; test `both_scoring_policies_increment_conn`); the
replaced estimator incremented only on the `LeastConn` branch.

### Clock — read fresh at selection, injectable in tests

`CompiledSelector::select` takes `now_units: u32`; production callers in
`backends/selector.rs` fill it with `crate::lb::ewma::score::now_units()`.

`upstream.start_time` is refreshed per attempt by `push_upstream`, so threading
it would in fact be correct. Reading fresh was chosen for simplicity — it avoids
plumbing an `Instant` through four call sites for a ~20 ns vDSO read that is
negligible beside connection setup. What must **never** be threaded here is
`ctx.start_time` or `ctx.upstream_start_time`: those are request-scoped, set
once, and on a retried or long-lived streaming request they are stale by
multiple half-lives, which would under-decay every score.

The parameter is kept rather than reading the clock inside `select` so tests can
inject deterministic time. `units_at(instant)` remains available where a caller
genuinely holds an `Instant` for *this* moment — the response filter reuses the
one it already took for header-time accounting, so `ht` and the LB sample are
provably the same measurement.

## Divergence summary

| Dimension | Tower / Linkerd | Proposed | Reason |
|---|---|---|---|
| Storage | Tower `Arc<Mutex<_>>`; linkerd `RwLock<Ewma>` | packed `AtomicU64` + CAS | P2C reads two backends per selection; no locks on the request path. Linkerd's pure `get_at` shows the read need not write back. |
| Decay target | 0 (both Tower and linkerd) | `default_rtt` | "no information" must be neutral, not winning |
| Decay math | `f64::exp` | Q16 table + shift | no float, no libm on the hot path |
| Smoothing weight | = decay factor | same (alpha deleted) | adopt; removes a dead global knob |
| Peak asymmetry | slower sample adopted whole | same | adopt |
| Pending count | `Arc::strong_count` | existing `conn_count` | we already have the counter |
| Sample endpoint | response headers | same | adopt; fixes SSE mis-scoring |
| Section header | "Proposed" | shipped 2026-09-04 | the column below is retained as the design record |
| Clock source | per-`load()` `Instant::now()` | fresh read at selection, passed as a parameter | Threading the request's start `Instant` was tried and rejected: it is stale by multiple half-lives on retries and long-lived streams. The parameter keeps time injectable in tests. |
| Error responses | recorded like any sample | recorded **peak-only** (`observe_peak_only`) | An error's elapsed time is not useful work. Blending a fast 5xx would give the fastest-failing backend the best score and pull traffic into a black hole. |
| Parameters | per-backend from control plane | hardcoded for now | no benchmark evidence yet; see below |

## Deliberately not doing (yet)

Exposing `halfLife` / `defaultRtt` as `EdgionBackendTrafficPolicy` fields. This follows the
precedent in [../04-review/lb/p2c-choice-count-fixed-at-two.md](../04-review/lb/p2c-choice-count-fixed-at-two.md): do not copy an upstream knob
into the CRD before benchmarks show operators need to turn it. Add the fields later if
evidence appears.

## Why this algorithm

The requirement that selected it: **an endpoint that stops receiving traffic must still be able
to recover.** The replaced fixed-alpha estimator moved only when a sample arrived, so a backend
that lost one P2C matchup lost all of them, forever. Any replacement therefore had to make time,
not traffic, the thing that forgets.

Given that, the rest follows from prior art. Peak-immediate adoption of a worse sample comes
from Finagle/Tower and is what makes the estimator react to degradation within one request
instead of over a window. Decaying toward `default_rtt` rather than toward 0 is our divergence:
"no information" must score neutral, not best, or a long-idle backend wins every matchup until
its first sample lands. Multiplying by `conn + 1` is Tower's, and is what stops a burst from
piling onto the historically-fastest backend before its samples return.

What it deliberately does **not** optimize for: precision. Every constant is a placeholder, the
timestamp is quantized to 16 ms, and the decay factor is a 33-entry fixed-point table. The
estimator only has to order two randomly sampled candidates correctly; it is not a latency
telemetry source, and `skills/03-coding/observability/` owns that.

## Known limitations

Accepted failure modes, each with its operational symptom.

1. **A faster sample arriving within the same 16 ms tick is discarded.** The blend weight is the
   elapsed-derived decay factor, so at zero elapsed the new sample's weight is exactly zero.
   Linkerd has the identical property, hidden behind nanosecond timestamps. At high QPS this
   means only peaks register and recovery is carried entirely by time-decay — which is the
   designed recovery path, so it is accepted. Do **not** floor the sample weight to "fix" it:
   that reintroduces the QPS-dependent smoothing this design removes. Asserted by
   `faster_sample_at_zero_elapsed_is_ignored`.
2. **`ts_units` wraps after ~2.2 years of process uptime, and the failure is silent.** `to_units`
   casts `millis / 16` to u32. Past the wrap, `now_units` restarts near zero while stored
   timestamps are near `u32::MAX`, so `now_units.saturating_sub(ts)` yields 0 for every backend:
   decay stops, faster samples carry zero weight, and each score freezes at its last peak until
   that backend's next `observe` re-anchors it — permanently for one that receives no further
   samples. **Operational symptom**: after multi-year uptime the LB permanently avoids backends
   that once spiked, with green health checks and nothing in the logs. Mitigation today is
   process lifetime; a real fix is wrapping arithmetic with a signed elapsed computation.
3. **A persistently fast-failing endpoint converges to neutral, not to bad.** Its peak-only
   writes are always skipped and time-decay pulls it to `DEFAULT_RTT_US`, while fast failures
   release their connection slot immediately so `conn` stays near zero. Its score is therefore
   ≈ 30 ms, which beats every healthy peer slower than 30 ms — i.e. most real services. The
   peak-only rule attenuates the black hole from "2 ms vs 40 ms" to "30 ms vs 40 ms"; it does
   not close it. The complete answer is outlier detection plus
   [lb-overload-penalty.md](lb-overload-penalty.md), not more scoring cleverness.
4. **`observe_peak_only` serves two callers with different input contracts.** The 5xx /
   route-retryable path passes raw measured time-to-headers clamped by
   `lb::sample::MAX_ERROR_SAMPLE_US`; the planned overload penalty will pass a bounded synthetic
   duration derived from `Retry-After`. The mechanism is genuinely shared — store only if it
   would raise the estimate — but the method bounds nothing itself, so every caller owns its own
   ceiling.
5. **Errors can raise a score but never lower one, so the estimate is asymmetric by design.**
   A backend that has stopped erroring still relaxes to neutral on the normal decay schedule, so
   this cannot inflate a score permanently — but during an error burst the score reflects the
   worst observation rather than the current one. That is the intended conservatism.
6. **A backend faster than `DEFAULT_RTT_US` is affinely compressed toward the default.** The
   high-QPS fixed point of `observe` on a below-default sample `s` is
   `v* = (DEFAULT_RTT_US · d + s) / (1 + d)`, so a 5 ms backend under ~60 rps scores ~18 ms
   while the same backend at 0.01 rps scores near 5 ms. Backends slower than default are
   adopted whole and are unaffected. This is the cost of decaying toward neutral rather than
   toward 0 (Tower/Linkerd's multiplicative compression). It is why `DEFAULT_RTT_US` must be
   chosen against mixed request rates, not only against a single QPS. `decay_is_rate_independent`
   checks the high-`d` band the old fixed-alpha estimator failed in; it does not claim
   `v*` is independent of rate.

## Replacement criteria

The proposal is adopted only if benchmarks show all of:

1. A spiked backend rejoins selection within `log2((sample − default_rtt)/default_rtt) ± 1`
   half-lives, from a deterministic injected-time test that states the sample and `default_rtt`
   it used. Unit coverage exists (`spiked_backend_recovers_without_traffic`,
   `spiked_backend_recovers_under_sustained_traffic`, `decay_is_rate_independent`); what
   remains for this criterion is the benchmark, not the test.
2. Steady-state distribution across equal-latency backends is no worse than today.
3. No measurable `select()` regression at backend counts 2 / 32 / 512.
4. SSE-shaped traffic no longer drives the estimate into the tens of seconds.

Open value questions for the benchmarks:

- **`default_rtt`.** Shipped at `DEFAULT_RTT_US = 30_000` (30 ms), matching Linkerd. The
  replaced estimator seeded 1 ms, which under peak semantics plus the pending multiplier
  over-favours new and idle backends. 30 ms is a placeholder adopted from prior art; the final
  value should come from measurement, not taste.
- **`half_life`.** Starting point 10 s, corroborated independently by Linkerd's `decay` default
  and Kong's `DECAY_TIME` in `kong/runloop/balancer/latency.lua`. It is **shared** with the
  overload penalty in [lb-overload-penalty.md](lb-overload-penalty.md), where it is also the
  bleed-off rate of a 429 penalty, so it cannot be tuned for latency alone. Benchmarks must
  report penalty-recovery time alongside latency-recovery time.

Note that recovery from a spike takes `log2((sample − default)/default)` half-lives, not one:
a 30 s SSE-shaped sample against a 30 ms default needs `log2(1000) ≈ 10` half-lives (~100 s) to
return to **parity**, which throughout this entry means twice a healthy peer's score. Criterion
1 above must therefore state the measured multiple of `H`, never "roughly one half-life"; it is
worded accordingly.

## References

- Task: `tasks/todo/time-decayed-peak-ewma-load-balancing.md`
- Dependent task: `tasks/todo/retry-after-backend-selection-penalty.md` — the provider
  `Retry-After` penalty is sequenced **behind** this one and folds into this scalar; its design
  record is [lb-overload-penalty.md](lb-overload-penalty.md)
- Sampler decision: [../04-review/lb/p2c-choice-count-fixed-at-two.md](../04-review/lb/p2c-choice-count-fixed-at-two.md)
- Overflow guard: [../04-review/lb/update-ewma-u128-intermediate.md](../04-review/lb/update-ewma-u128-intermediate.md)
- Tower: <https://tower-rs.github.io/tower/src/tower/load/peak_ewma.rs.html>
- linkerd2-proxy balance: <https://github.com/linkerd/linkerd2-proxy/blob/a66af811/linkerd/proxy/balance/src/lib.rs>
- linkerd2-proxy ewma: <https://github.com/linkerd/linkerd2-proxy/blob/a66af811/linkerd/ewma/src/lib.rs>
- linkerd2-proxy load-biaser: <https://github.com/linkerd/linkerd2-proxy/blob/a66af811/linkerd/load-biaser/src/lib.rs>
- Envoy Peak EWMA: <https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/contrib/load_balancing_policies/peak_ewma/peak_ewma>
