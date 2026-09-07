---
name: lb-overload-penalty
description: Provider overload feedback in load-balancer scoring — how a qualifying upstream 429 (or 503 with Retry-After) becomes a bounded absolute synthetic latency sample inside the Peak-EWMA scalar, why both the lexicographic-tuple and the default_rtt-relative sizings were rejected, and the Linkerd/Envoy/Kong/Finagle comparison that settled it.
status: proposed
---

# Overload penalty scoring — load balancing

> **Prerequisite landed 2026-09-04.** The time-decayed Peak-EWMA estimator ships, and with it
> the score-side primitive this design needs — now named `observe_peak_only` (store only if it
> would raise the estimate; no timestamp advance otherwise). It already has one production
> caller: the 5xx / retryable-status path in `lb::sample`. **No 429 caller exists** — the
> `Retry-After` parse and clamps, the `UpstreamInfo` switch, and the CRD field remain to be
> written. See `tasks/todo/retry-after-backend-selection-penalty.md`.

An upstream that answers `429 Too Many Requests` is telling us something the latency estimate
cannot see: it is shedding load *cheaply*. A rate-limit rejection is usually fast, so it
**improves** the backend's latency score, and P2C then steers more traffic at the endpoint
least able to serve it.

This entry covers the signal and how it enters the score. The estimator it folds into is
[lb-peak-ewma.md](lb-peak-ewma.md); the sampler is
[../04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md](../04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md).

## What we run today

The store primitive ships as `observe_peak_only`, with exactly one production caller: the
5xx / route-retryable path in `lb::sample::record_response_sample`, which prevents a
fast-failing backend from acquiring the best score (see [lb-peak-ewma.md](lb-peak-ewma.md)).

That caller already covers a **429 listed in the route's `retry.codes`** — such a 429 is
classified as not-useful-work and can no longer lower a score. What is still missing is the
429-specific *penalty*: honouring `Retry-After`, the bounded clamp, and the per-Service switch.
Without it the signal is merely neutral rather than counted backwards, and for a 429 that is
**not** in `retry.codes` the gap remains:

- `FailureClass::from_http_status`
  (`edgion-gateway/src/backends/outlier_detection/state.rs:18-26`) returns `None` below 500, so
  a 429 reaches `record_success_by_addr` in `pg_upstream_response_filter.rs` (the generic outlier block) and is
  recorded as a **success** for outlier detection. `PassiveEjectionState::record_success`
  (`state.rs:256`) then zeroes `app_failures` / `gateway_failures` /
  `local_origin_failures` **and clears `open_until`**. An endpoint alternating 503 and 429
  therefore never accumulates `consecutive_errors` and can never be ejected: its 429s actively
  protect it from the mechanism that should remove it.
- The `Ewma` policy records the attempt's elapsed time with no status input, so a fast 429
  also lowers the latency score.

The AI path is the precedent, not the model: `edgion-gateway/src/ai/credential_outlier.rs`
applies a bounded `Retry-After` cooldown, but its identity is `(backend key, credential alias)`
and its mechanism is `PassiveEjectionState::open_for` — a real open/closed state, not a score.
Endpoint-scoped scoring is a different problem and must not reuse that type.

## Proposed algorithm

**A qualifying failure records a synthetic latency sample instead of its measured elapsed
time.** No new per-endpoint state, no deadline, no change to the P2C comparator.

The penalty is an **absolute duration**, clamped. A `default_rtt`-relative sizing was tried and
rejected; [Sizing](#sizing-why-absolute-and-not-a-multiple-of-default_rtt) records why, because
it is plausible enough to be re-proposed. (A lexicographic P2C comparator was also rejected,
but that is a different mechanism, not a sizing — see
[Why this algorithm](#why-this-algorithm).)

```text
at upstream response headers, when the Service's policy opts in:

  raw     = header value, rejected if longer than 64 bytes
  hint    = parse_retry_after(raw, now)          # canonical parser; None if absent/malformed
  penalty = hint.unwrap_or(BASE_PENALTY)
                .clamp(BASE_PENALTY, MAX_PENALTY)   # 5 s .. 300 s

  if penalty <= read(now) { return }              # not a peak: skip entirely, see below
  store_peak(penalty, now)                        # peak branch, truncated ts
```

Everything after that is the ordinary estimator: the score is `read(now).saturating_mul(conn + 1)`
and the penalty bleeds off through the same half-life as any other slow sample. Selection,
health filtering, circuit-breaker permits, panic mode, and retry budgets are untouched.

### Sizing: why absolute and not a multiple of `default_rtt`

Under `read(now) = default + (stored − default) · 2^(−t/H)`, the time for a stored value `P` to
decay to a multiple `m` of `default` is `t = H · log2((P − default) / ((m − 1) · default))`.
Throughout this entry **parity means `m = 2`** — still twice a healthy peer's score. Stating it
matters: at `m = 1.1` every recovery figure below roughly doubles.

Recovery time therefore does depend on `default_rtt`, but only logarithmically:

| `default_rtt` | `BASE_PENALTY` = 5 s | time to parity |
|---|---|---|
| 30 ms | 5 s | 73.7 s (≈ 7.4 half-lives) |
| 1 ms | 5 s | 122.9 s (≈ 12.3 half-lives) |

A **30× change in `default_rtt` moves recovery by 1.7×**. That is weak enough to accept and
document rather than engineer around, and the acceptance criterion simply has to state the
measured multiple of `H` instead of asserting "roughly one half-life".

**Rejected: `penalty = default_rtt · 2^lives`.** Making the penalty a multiple of `default_rtt`
does remove the log-coupling of recovery time — and replaces it with a *linear* coupling of
penalty **strength**, which is the property that actually matters. `default_rtt` is a floor
constant, not a typical score: real endpoints score far above it, and `conn + 1` is a
dimensionless count that does not scale with it. Concretely, at `BASE = 3` lives the penalty is
`8 · default_rtt`; against the 1 ms seed the replaced estimator used, a penalized endpoint
would score 8 ms and still **beat** a healthy 30 ms peer at zero in-flight. The shipped
`DEFAULT_RTT_US` is now 30 ms, which softens that particular arithmetic — but the coupling
itself is the objection and it is unchanged: penalty strength still scales linearly with a
benchmark-gated constant, while `conn + 1` does not scale with it at all. Worse, the peak branch means
the write is not even stored whenever the endpoint's own score already exceeds `8 · default_rtt`
— which is most real backends. Measured as "how many competitor in-flight requests the penalty
outweighs":

| `default_rtt` | competitor latency | absolute 5 s | `8 × default_rtt` |
|---|---|---|---|
| 1 ms | 30 ms | 166 | **0.27** — inert |
| 30 ms | 30 ms | 166 | 8 |
| 30 ms | 300 ms | 16 | **0.8** — inert |

A ratio sizing is also unable to touch the population this feature cites as its motivation: a
partially-shedding endpoint is serving real traffic, so its score sits at real latency, orders
of magnitude above any small multiple of `default_rtt`. There is no formulation in which a
`default_rtt`-relative penalty and a count-based `conn + 1` multiplier are jointly scale-free.

**The hint is honored logarithmically, and that is not fixable.** `Retry-After: 60` and
`Retry-After: 300` differ by `log2(5) ≈ 2.3` half-lives — about 23 s of exclusion. Encoding
"exclude for exactly `h` seconds" would require storing `default · 2^(h/H)`, which for
`h = 300, H = 10` is ~3.2e7 s — far past the u32 µs field. Clamping the hint into a narrow
window to buy linearity only trades this for a *constant* response outside the window, which is
worse. State the compression; do not engineer around it.

### Bounds and why each exists

| Bound | Value | Guards against |
|---|---|---|
| 64-byte header cap before parsing | `MAX_PROVIDER_RESET_HINT_BYTES` | Handing attacker-controlled bytes of unbounded length to `httpdate::parse_http_date`. The AI path already applies this cap; the generic path must too. |
| `clamp(.., MAX_PENALTY)` | 300 s | An upstream de-prioritizing itself for an unbounded period with one header. `Retry-After` is bug- or attacker-controlled input, and `parse_retry_after` clamps nothing — an HTTP-date one year out is 3.15e13 µs, 7300× past the u32 µs field. **This clamp is what discharges the overflow hazard**, under any sizing. |
| `clamp(BASE_PENALTY, ..)` | 5 s | A `Retry-After: 1` making a rate-limited endpoint look **healthier** than a rate-limited endpoint that sent no header. The hint may only lengthen the penalty, never shorten it. |
| **Skip the write entirely when `penalty <= read(now)`** | — | See below. |

The last row is the subtle one, and an earlier revision got its justification wrong in a way
that invited a real bug. The wrong version claimed that at write time `elapsed = 0`, so the
estimator's blend returns the current value and `store_peak(P)` equals `store(max(P, read))`.
That conflates the write's own instant with the estimator's `elapsed`, which is measured from
the **previous** write and is essentially never zero. With `d = 2^(−elapsed/H) < 1`, an
ordinary `update(P)` for `P < eff` yields `eff + (P − eff)(1 − d)` — it **strictly lowers** the
score toward `P`, at a rate of `ln2 / H` per unit time regardless of QPS. A 429 storm would
drag a genuinely slow endpoint's score down to the penalty value, resurrecting the exact defect
this feature exists to fix: 429s improving the score.

So the requirement is an explicit early return — no store, no timestamp advance — not a `max()`
and not a routing of the penalty through the shared `update()`.

### Repeated penalties are idempotent, not additive

The early return implements `max` semantics without a `max` write: skipping leaves the current
value in place, which *is* `max(P, read(now))` whenever `read(now) > P`. So the value observed
at successive 429s follows `v_{k+1} = max(P, default + (v_k − default) · 2^(−Δ/H))`: strictly
decreasing while `v_k > P`, then pinned at `P`. Repeatedly 429-ing an endpoint already scoring
worse than the penalty converges *down* to `P`; it does not hold the endpoint at its worst
observed value. There is no absorbing state and no back door into one.

Skipping also means the decay **anchor is not reset** on a non-peak 429. Decay therefore runs
continuously from the last real sample — `2^(−Δ₁/H) · 2^(−Δ₂/H) = 2^(−(Δ₁+Δ₂)/H)`, so the
recurrence above still composes correctly — and the penalty path performs no Q16
recomputation at all, which is what keeps it out of the value-drift bound below.

When a penalty *is* stored, it holds **only** if the write anchors on a **truncated** `now`.
Timestamp truncation errors
then telescope — `Σ(floor(t_i) − floor(t_{i−1})) = floor(t_n) − floor(t_0)` — so total
*timestamp* drift is bounded by one quantization unit regardless of write count. An
implementation that stores true-`now` while computing elapsed from a truncated anchor compounds
the error per write, and a high-QPS 429 storm *will* pin the score.

**The telescoping bound covers timestamps only, not values.** Each decay recomputation loses up
to `2^−16` relatively to the Q16 floor, and that loss compounds multiplicatively across writes:
65,536 re-stores drive the value to `e^−1 ≈ 0.37` of where elapsed time alone would put it — a
63% unjustified downward drift, reachable in ~65 s at 1,000 writes/s. The skip-if-not-a-peak
rule above removes the penalty path's contribution to this entirely, which is a second reason
to prefer the early return over a `max()`. The ordinary sample path still needs its own
decision, recorded in [lb-peak-ewma.md](lb-peak-ewma.md) — and note it is in tension with that
entry's invariant 5, which forbids round-half-up.

Note also that this proof is independent of how the penalty is sized: it holds for both the
absolute and the `default_rtt`-relative forms, so it is not evidence for either.

### Qualifying responses

| Response | Penalized | Why |
|---|---|---|
| `429`, with or without `Retry-After` | Yes | The only status whose meaning is "you are sending too much". A bare 429 is still an unambiguous overload statement, and most real servers do not send the header — penalizing only hinted 429s would make the feature inert against the majority of backends. |
| `503` **with a valid `Retry-After`** | Yes | RFC 9110 §15.6.4 pairs exactly this with load shedding. See [Why 503 needs a graded signal too](#why-503-needs-a-graded-signal-too). |
| Bare `503`, other `5xx` | No | Generic server errors. Outlier detection's ejection is the right response. |
| Other `4xx` | No | Client errors. Not endpoint state. |
| gRPC `RESOURCE_EXHAUSTED` (status 8) | Not in v1 | Linkerd penalizes it. Deferred — see [Known limitations](#known-limitations) item 5. |

### Companion change: 429 must become neutral for outlier detection

Because a 429 currently calls `record_success` and thereby clears `open_until` and every
failure counter, adding the penalty without also fixing this leaves the endpoint *better*
protected from ejection than before. The same change must make 429 record **neither** success
nor failure for outlier detection. This is one branch, and it is the natural companion to
"429 is not a health verdict, it is a score input".

### Why 503 needs a graded signal too

An earlier revision excluded 503 on the grounds that it would double-count with outlier
detection. **That reasoning is wrong**: an ejected endpoint is removed by `health_filter`
inside `select_p2c`, so its score is never consulted. Score and ejection are mutually exclusive
in effect and cannot double-count.

The real cost of excluding 503 is two quantified gaps:

- **Sub-threshold.** `consecutive_errors` defaults to 5, so consecutive 503s 1 through 4
  produce zero de-prioritization — the endpoint keeps winning P2C matchups at full rate while
  explicitly declaring overload. Any single success resets the counter, so an endpoint shedding
  at 50% never ejects *and* never gets deprioritized.
- **Percent cap.** `max_ejection_percent` defaults to 50. Under a fleet-wide overload — the case
  this feature exists for — the guard denies ejection and the operator gets nothing at all. A
  score has no such cap and needs none, because penalties do not remove endpoints.

Restricting it to 503 **with a valid `Retry-After`** keeps the population narrow and explicitly
self-declared, disjoint from the generic-error traffic outlier detection responds to.

### Constants

| Constant | Value | Derivation |
|---|---|---|
| `BASE_PENALTY` | 5 s | Linkerd's `DEFAULT_PENALTY_MS`. Adopted as-is: it is the only value in this design with a shipped prior implementation behind it. Yields ≈ 74 s to parity at a 30 ms `default_rtt`, ≈ 123 s at 1 ms. Benchmark-gated against our own `default_rtt`. |
| `MAX_PENALTY` | 300 s | Linkerd's `DEFAULT_RETRY_AFTER_MAX_DURATION`; Envoy's `RateLimitedRetryBackOff.max_interval` and `max_ejection_time` independently land on the same number, though there it bounds a retry delay and an ejection, not a score. Because the hint is honored logarithmically, raising this to 300 s from 60 s buys only ≈ 23 s of additional exclusion — it is a safety bound, not a tuning knob. |
| `HALF_LIFE` | 10 s | Owned by [lb-peak-ewma.md](lb-peak-ewma.md); matches Linkerd's `decay` and Kong's `DECAY_TIME`. **Shared** — it is also this penalty's bleed-off rate, so it cannot be tuned for latency alone. |

`BASE_PENALTY` and `default_rtt` are not independent: recovery is `H · log2(P / default_rtt)`.
The two benchmarks must be read together, and the penalty benchmark must state the
`default_rtt` it ran against.

Only the on/off switch is exposed as an `EdgionBackendTrafficPolicy` field, defaulting to off.
The magnitudes stay hardcoded pending benchmarks, per the precedent in
[../04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md](../04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md).

## Contract

| Property | Value |
|---|---|
| New per-endpoint state | **None.** Reuses the Peak-EWMA `AtomicU64` via `upstream.lb_backend`. |
| New per-attempt state | One resolved boolean on `UpstreamInfo`, alongside the existing `lb_policy`. Required because `selected_effective_policy()` returns `None` for `SelectedHttpBackend::Service`, so the generic path has no policy snapshot to consult at response time. Resolving it at peer selection keeps the response path free of map lookups and keeps the switch stable across a mid-request config swap. |
| Read cost | Unchanged — the penalty is invisible to the read path. |
| Write cost | On a qualifying response only: one bounded header parse plus one CAS. |
| Concurrency | Lock-free. `read(now)` and the resulting `penalty` **must be recomputed inside the CAS retry loop**; hoisting them out can stomp a concurrent higher peak. `PeakEwmaScore::observe` / `observe_peak_only` are the shipped reference shape. Concurrent penalties are idempotent — two simultaneous 429s cannot compound. |
| Saturation | The store boundary must convert to u32 µs **saturating**, never `as u32`. `MAX_PENALTY = 300 s` = 3e8 µs fits, but the clamp is the *only* thing preventing a wrap: raise it past ~4295 s and `as u32` turns a longer hint into a shorter penalty, silently. The `conn + 1` multiply is `saturating_mul`, as the estimator specifies. |
| Clock | The **score** is monotonic-only. `parse_retry_after` needs a `SystemTime` for the HTTP-date branch; that wall-clock read produces a `Duration` and never becomes state. |
| Retained data | A duration. No response bodies, provider text, addresses, or credentials — the raw header value never enters state or any observability surface. |
| Determinism | Injected `now`, same as the estimator. |
| Applies to | The `Ewma` policy **only**. `LeastConn` scores on `conn()` and `ConsistentHash` / RoundRobin do not score at all, so an EWMA-resident penalty is inert under every other policy. Note `ParsedLBPolicy` has no `RoundRobin` variant — RoundRobin is `None`. Configuring the switch under a non-`Ewma` policy is a validation WARN, never a rejection. |

## Why this algorithm

The requirement that selected it: **the signal must be advisory.** A rate-limited endpoint is
still a working endpoint, and when a quota is shared across a fleet, every endpoint is
rate-limited at once. Any mechanism that removes endpoints degrades, at saturation, to
"no endpoints available".

Folding into the existing scalar is what makes that fall out for free. There is nothing to
decide about the all-penalized case, because a penalty is just a large latency and the
comparator is unchanged.

### Rejected: a lexicographic tuple `(remaining_penalty_ms, base_metric)`

The first design stored a bounded monotonic deadline per endpoint and compared P2C candidates
on the tuple. Rejected for four reasons, three of them behavioral:

1. **It is "strictly last", not "sorts last".** Lexicographic ordering makes any nonzero
   penalty dominate every base-metric difference: an endpoint with 1 ms of penalty remaining
   loses to one timing out at 30 s. That is an ejection with a countdown, placed in the
   selection path where it is harder to observe than an ejection.
2. **Cliff at expiry.** At the deadline the endpoint jumps from strictly-last to fully
   competitive in one step with no residual memory. Under sustained 429s it oscillates:
   penalized → expire → hammered → 429 → penalized. Exponential decay has no cliff.
3. **Degenerate under correlated load.** When every endpoint is penalized — the common case,
   since rate limits are usually per-account rather than per-replica — the first component is
   nonzero everywhere and becomes the *only* selector. Latency and connection count are
   ignored precisely when they matter most.
4. **No exchange rate, so nothing to tune.** A scalar answers "a 3-half-life penalty ≈ being
   8× slower than a healthy endpoint". A tuple hardcodes "never prefer a penalized endpoint",
   with no knob and no way to state the tradeoff.

**The tuple was a symptom, not a choice.** It existed because the estimator at the time was
fixed-alpha: decay is driven by sample arrival, and a penalized endpoint receives no samples
*because* it is penalized, so the penalty would never decay. An explicit deadline was the only
way out. Making the estimator time-decayed removes the need for the deadline entirely, which
is why this task is sequenced behind `tasks/todo/time-decayed-peak-ewma-load-balancing.md`
rather than beside it.

## Prior art comparison

Linkerd was read from source at `linkerd/linkerd2-proxy@a66af811` (sparse-checkout of
`linkerd/ewma`, `linkerd/load-biaser`, `linkerd/pool/p2c`). Kong's balancer was read from
source. **Every other row is from upstream documentation only** and is marked `[docs]` — per
Rule 1 of [SKILL.md](SKILL.md), treat those verdicts as weaker evidence than the source-read
rows.

| Implementation | 429 → selection? | Mechanism | Graded? | Our verdict |
|---|---|---|---|---|
| **Linkerd Load Biaser** `[source]` | Yes | Synthetic RTT sample into the Peak-EWMA scalar; `effective_rtt = max(clamp(Retry-After, 300 s), 5 s)` | Yes | **Adopt the shape and the magnitudes.** The one divergence is the store discipline: we skip the write when it would not be a peak, rather than relying on `max()`. |
| **Finagle** `P2CPick` `[docs]` | No | Compares `Status` first (`Open`/`Busy`/`Closed`), then load — a two-tier comparator whose first tier is **binary availability**, never a graded penalty | Scalar only, below the tier | Confirms the rule we follow: binary availability is a separate tier or a readiness gate; everything continuous folds into one scalar. Our health filter *is* that tier. |
| **Envoy** outlier detection `[docs]` | No — 429 is classified a **success** by default and improves the host's success-rate statistics | 5xx-driven binary ejection | No | Reject the classification; adopt the boundary (429 stays out of ejection). |
| **Envoy** `rate_limited_retry_back_off` `[docs]` | No | Parses `Retry-After` / `X-RateLimit-Reset`, but only to time **one request's retry**; leaves no residue on the endpoint | n/a | Confirms the parser; does not solve endpoint selection. Note its 300 s `max_interval` means "retry delay", not "exclusion time" — do not borrow the number's provenance. |
| **Envoy** `x-envoy-degraded` `[docs]` | No | Degraded hosts "remain in the rotation but are deprioritized", served only when healthy hosts are insufficient | Two-tier, not graded | Same philosophy, coarser: a hard priority tier rather than a score. |
| **Envoy** slow start / ORCA `client_side_weighted_round_robin` `[docs]` | No | Genuinely graded weight adjustment | Yes | Graded, but recovery-ramp or cooperative-utilization driven, never overload-status driven. |
| **Kong** passive health checks `[docs]` | Yes | Default `unhealthy.http_statuses` includes 429 → target **removed** from the ring balancer | No | Reject. This is the failure mode we exist to avoid: all-429 empties the ring and returns 503. |
| **Kong** `latency` LB `[source]` | No | Peak-EWMA + P2C, `DECAY_TIME = 10 s`, `PICK_SET_SIZE = 2`, deliberately status-blind | n/a | Same base algorithm as ours; confirms status input is an addition, not a redesign. Source of our `HALF_LIFE = 10 s` corroboration. |
| **NGINX** `max_fails` / `fail_timeout` `[docs]` | Yes, via `proxy_next_upstream http_429` | Binary unavailability for `fail_timeout`; weight is static config | No | Reject. |
| **HAProxy** `observe layer7` / `on-error mark-down` `[docs]` | No (no configurable status list) | Binary DOWN. `slowstart` is graded but recovery-only, static duration | No | Reject. |
| **Traefik** CircuitBreaker `[docs]` | Expressible via `ResponseCodeRatio(429,430,…)` | Service-scoped, not endpoint-scoped; trips the whole service to 503 | No | Reject — wrong granularity. |
| **LiteLLM** router `[docs]` | Yes | Deployment cooldown; all-cooled raises `RouterRateLimitError` | No | Reject; documents the saturation failure mode concretely. |

**The industry gap this fills.** Before Linkerd's Load Biaser, the proxies that *read*
`Retry-After` (Envoy) and the proxies that *treated 429 as an endpoint signal* (Kong, NGINX)
were disjoint sets — nobody joined them, and everyone who acted on 429 did so by removing the
endpoint. Linkerd is the first to make it graded and advisory. We are adopting a shape with
exactly one prior implementation, which is worth knowing when weighing confidence.

### Divergences from Linkerd

| Dimension | Linkerd | Ours | Reason |
|---|---|---|---|
| Penalty magnitude | `5 s` base, `300 s` cap | Same | Adopted unchanged. A `default_rtt`-relative sizing was tried and rejected — see [Sizing](#sizing-why-absolute-and-not-a-multiple-of-default_rtt). |
| Storage | `RwLock<Ewma>`, read under a read-lock in `load()` | Packed `AtomicU64` + CAS | Inherited from [lb-peak-ewma.md](lb-peak-ewma.md): P2C touches two backends per selection. The reason is *no lock at all*, not read-purity — Linkerd's `Ewma::get_at(&self, now)` is already a pure, non-mutating projection. |
| Decay target | 0 | `default_rtt` | Inherited. "No information" must be neutral, not winning. |
| Qualifying statuses | 429, 503, gRPC 8 / 14 / {2,4,13,15} | 429; 503 only with a valid `Retry-After` | Bare 5xx is generic server error where ejection is the right response. A self-declared retry window is a different population. |
| Pending count | `Arc::strong_count` of the in-flight handle | Existing `conn_count` | Inherited. |
| Config surface | Per-Service annotations, all three values tunable | Per-Service CRD field, switch only | No benchmark evidence for the magnitudes yet. |
| AI/credential interaction | No equivalent | Endpoint penalty **skipped** when a credential cooldown was applied for the same response | Provider quotas are per-credential; the endpoint address is shared across credentials. See limitation 2. |
| 429 during breaker probation | Strict — a 429 **fails** the probe even though the classifier calls it a success, so recovery backoff is not cut short while the endpoint is still shedding | Open question | Worth copying if our outlier detection grows a half-open probe. Recorded so it is not silently dropped. |

## Known limitations

Accepted failure modes, each with its operational symptom.

1. **Shared quotas make the signal ineffective.** If the rate limit is per-account or
   per-downstream-service rather than per-replica, penalizing one endpoint just rotates the 429
   around the fleet. The scalar degrades gracefully — all scores rise together, relative
   ordering still carries latency information — but the feature buys nothing. This is the
   stated scope condition and a reason for default-off.
2. **On the AI path a shared endpoint would be mis-attributed, so the penalty is skipped
   there.** Provider quotas are per-credential while the endpoint address is shared across all
   credentials for that provider, so credential A hitting its quota would deprioritize the
   endpoint for credentials B..N which are not throttled. When
   `AiCredentialOutlierHandle::apply_retry_after` accepted a cooldown for this response, the
   more specific identity has claimed the signal and the endpoint penalty must not also fire.
3. **`x-ratelimit-reset-*` is honored on the AI path but not here.** A provider sending only
   that header produces a credential cooldown and, on the generic path, only the base penalty.
   Reusing `parse_rate_limit_reset` as a secondary hint is a deliberate non-goal for v1; record
   the divergence rather than closing it silently.
4. **The request that received the 429 still fails.** The penalty affects future selections
   only. Envoy's companion move is `retriable_status_codes: [429]` plus a previous-hosts
   predicate so the in-flight request is immediately re-dispatched. Whether our retry policy
   should list 429 is a separate, deliberate decision — 429 is not always safe to retry.
5. **gRPC `RESOURCE_EXHAUSTED` is not a qualifying signal in v1.** Linkerd penalizes it, and a
   gRPC-heavy deployment gets nothing from this feature until it is added. Deferred rather than
   rejected; tracked in `tasks/todo/retry-after-backend-selection-penalty.md`.
6. **The magnitude is the upstream's own claim.** Unlike latency or connection count, this
   signal is self-reported. The `MAX_PENALTY` clamp bounds the damage; it does not make the
   signal trustworthy. No cap on *how many* endpoints may be penalized at once is needed,
   because penalties do not remove endpoints — but that invariant is load-bearing and is why
   the tuple shape was unacceptable.
7. **Severity is inversely coupled to load at penalty time, and bounded by peer concurrency.**
   Because the score is `read(now) * (conn + 1)`, an endpoint holding many in-flight requests
   when the 429 lands sees its score drop sharply as `conn` drains, discounting the penalty —
   self-limiting re-admission, mostly desirable. The flip side is a ceiling on strength: a 5 s
   penalty is outweighed once a healthy peer at latency `L` carries ~`5 s / L` in-flight
   requests. On an AI gateway where `conn` counts long-lived SSE streams, 166 concurrent
   streams per endpoint is ordinary, so under that traffic shape the penalty is a tiebreak
   rather than a strong signal. Benchmarks must fix and report in-flight depth.

## Replacement criteria

Adopted only if, on top of the Peak-EWMA criteria:

1. A load test with one persistently rate-limited endpoint shows a measurable drop in repeated
   429 selection versus the unpenalized baseline.
2. A recovered endpoint returns to **parity (`m = 2`, i.e. twice a healthy peer's score)**
   within `log2(BASE_PENALTY / default_rtt) ± 1` half-lives of its last qualifying response,
   from injected time — ≈ 7.4 half-lives at a 30 ms `default_rtt`, ≈ 12.3 at 1 ms. The
   criterion states a formula and the `default_rtt` it was measured against, never "roughly one
   half-life".
3. The all-penalized case shows selection still distributed by the base metric, not collapsed
   onto one endpoint.
4. A repeated-429 storm converges to `BASE_PENALTY` and does not pin the score at a worse value
   — the telescoping-timestamp invariant, asserted directly.
5. Benchmarks include an SSE-shaped high-concurrency case, and report the in-flight depth they
   ran at. The `conn + 1` multiplier means a 5 s penalty is neutralized once a healthy peer at
   latency `L` carries about `5 s / L` in-flight requests — 166 at 30 ms, 16 at 300 ms. The
   existing 2 / 32 / 512 *backend* sweep does not exercise in-flight depth at all.
6. No measurable `select()` regression — expected, since the read path is unchanged.

If (1) cannot be demonstrated, the likely cause is limitation 1 (shared quota) in the harness,
not the algorithm. Record which, and re-run against a per-replica limit before rejecting.

## References

- Task: `tasks/todo/retry-after-backend-selection-penalty.md`
- **Prerequisite** task: `tasks/todo/time-decayed-peak-ewma-load-balancing.md`
- Estimator: [lb-peak-ewma.md](lb-peak-ewma.md)
- Sampler: [../04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md](../04-review/lb/intentional-design/p2c-choice-count-fixed-at-two.md)
- Overflow precedent: [../04-review/lb/superseded/update-ewma-u128-intermediate.md](../04-review/lb/superseded/update-ewma-u128-intermediate.md)
- Canonical parser: `edgion-common/src/time.rs::parse_retry_after` — accepts delay-seconds or
  HTTP-date, returns `Some(Duration::ZERO)` for a past date, and **clamps nothing**. Every
  bound above is the caller's responsibility.
- Credential-scoped precedent (different identity, different mechanism):
  `edgion-gateway/src/ai/credential_outlier.rs`
- Linkerd Load Biaser: <https://github.com/linkerd/linkerd2-proxy/blob/a66af811/linkerd/load-biaser/src/lib.rs>
- Linkerd EWMA: <https://github.com/linkerd/linkerd2-proxy/blob/a66af811/linkerd/ewma/src/lib.rs>
- Linkerd docs (annotations and defaults): <https://linkerd.io/docs/reference/load-balancing/>
- Finagle `P2CPick` (unpinned — `develop` will drift; re-verify before citing): <https://github.com/twitter/finagle/blob/develop/finagle-core/src/main/scala/com/twitter/finagle/loadbalancer/p2c/P2CPick.scala>
- Envoy outlier detection: <https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/outlier>
- Envoy `RateLimitedRetryBackOff`: <https://www.envoyproxy.io/docs/envoy/latest/api-v3/config/route/v3/route_components.proto>
- Envoy degraded hosts / slow start / ORCA: <https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/load_balancing/slow_start>
- Kong health checks and circuit breakers: <https://developer.konghq.com/gateway/traffic-control/health-checks-circuit-breakers/>
- Kong `latency` balancer (unpinned — `master` will drift; re-verify before citing): <https://github.com/Kong/kong/blob/master/kong/runloop/balancer/latency.lua>
- NGINX upstream module: <https://nginx.org/en/docs/http/ngx_http_upstream_module.html>
- HAProxy circuit breakers: <https://www.haproxy.com/documentation/haproxy-configuration-tutorials/reliability/circuit-breakers/>
- Traefik CircuitBreaker: <https://doc.traefik.io/traefik/v3.4/middlewares/http/circuitbreaker/>
- LiteLLM routing: <https://docs.litellm.ai/docs/routing>
