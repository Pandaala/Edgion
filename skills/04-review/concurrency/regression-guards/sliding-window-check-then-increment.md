---
name: sliding-window-check-then-increment
description: Review shared outbound atomic window/admission updates, monotonic expiry, delayed calls and representation bounds.
knowledge_type: regression-guards
---

# Outbound rate-limit window and admission coherence

## Conclusion and scope

`SlidingWindowCounter` in `edgion-gateway/src/link_sys/providers/webhook/runtime.rs`
is owned by each `ManagedOutboundClient`, including Webhook, ForwardAuth,
AiGuard and Guardrail clients with `rateLimit` configured. `WebhookRateLimit.rate`
becomes `limit`; `windowSec` becomes `window_secs`, floored to one second.
`outbound_call_with_body` checks it once before request construction and retries.
Rejection remains `OutboundCallError::RateLimited`; consumer failure policies
and configuration/client replacement behavior are unchanged.

One `AtomicU128` publishes the epoch and both counts. Admission commits rotation
and increment together; contention rechecks the entire state. This corrects
`webhook-sliding-window-epoch-coherence` and the overlapping clock/idle-expiry
claim in `D0019`. Required verification is recorded below.

## Rationale and evidence

Historical `link-sys-F006` (2026-05-25) replaced a separate check/increment with
a current-count CAS. It fixed fixed-epoch over-admission, but timestamp-first
rotation could still move new admissions into previous. Cached previous/ratio
evidence could survive count ABA; long idle retained expired history.
The `retry-budget-generation-and-window-coherence` investigation correctly
narrowed the historical claim without fixing this separate limiter.

The replacement samples monotonic whole-second elapsed time once per public
operation, from the instance's `Instant` origin. Fixed-grid math is:

- `epoch = elapsed_secs / window_secs`; zero is a valid initial epoch.
- `remaining = window_secs - elapsed_secs % window_secs`.
- Admit when `previous * remaining + current * window_secs < limit * window_secs`.

Products and sums use `u128`: two 40-bit counts times 64-bit durations and the
64-bit limit/duration product fit. This preserves pre-increment two-bucket
approximation, not an exact rolling event log, and removes floating-point
rounding. Advancing one epoch retains current as previous; advancing two or
more drops both. There is no separately visible timestamp, reset or rollback.

All writers CAS one word. Acquire loads supply complete snapshots; AcqRel CAS
validates the snapshot and is the admission linearization point. A failed CAS
supplies a fresh Acquire snapshot and repeats expiry/quota checks. Epochs never
wrap and counts only increase within an epoch, so equal words cannot conceal
rotation. Denied calls do not publish rotation; later calls derive expiry from
the last admitted epoch.

Authority: the user-approved Fix Impact for this task,
[time conventions](../../../03-coding/03-time-conventions.md) and
[graded shared-state rules](../../../00-design-rules.md). The shared-client
scope, delayed-call policy and representation bounds were explicitly accepted.

## Alternatives and limits

- A sampled epoch older than the published state denies. CAS retries retain the
  original sample; they never silently charge a newer epoch. A successful CAS
  may return after another thread advances: its admission already committed.
- The layout is 48 epoch bits and two 40-bit counts. Each bucket admits at most
  `2^40 - 1` calls even for a larger configured `u64` limit, then denies until
  the window advances. Previous remains an exact weighted count, not a sticky
  saturation sentinel. Epoch exhaustion denies for the instance without wrap.
- No production allocation, lock, test hook, tracing or new metric is added.
  The existing Gateway startup gate requires native `AtomicU128` support;
  amd64 VMs must expose CMPXCHG16B. CAS is lock-free, not wait-free.
- RetryBudget's separate epoch slots allow check/increment slack; they cannot
  implement this atomic admission contract. Reuse is limited to the dependency
  and CPU capability gate, without a new abstraction or retry-accounting change.
- The limiter is per client per Gateway, not a cluster quota or per retry attempt.

## Verification and re-evaluation

Fresh verification on 2026-09-07, macOS arm64:

- `cargo test -p edgion-gateway --lib link_sys::providers::webhook::runtime::tests -- --test-threads=1`:
  14 passed, including the final test-only synchronization correction.
- The freshly built Gateway test executable ran
  `plugins::http::common::outbound::call::tests::rate_limited_gate_returns_rate_limited_error --exact`:
  1 passed.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy retained existing
  warnings outside the two edited Rust files; neither edited file emitted one.
- Agent-doc consistency, SSA-force, metrics-inventory, Pingora-source policy,
  Gateway no-tracing and `git diff --check` passed.
- Independent read-only review: LGTM, including the final test-only Mutex
  correction, original knowledge sources, caller scope and accepted bounds.

Deterministic coverage includes pauses before/after CAS, epoch/count ABA,
competing admissions, one public-operation clock sample, fixed-grid weighting,
two-window/long-idle expiry, reversed samples, zero limits/windows, full-width
duration/limit arithmetic, count capacity and epoch exhaustion. Eight concurrent
workers admit exactly 500 calls when 1000 previous calls carry a one-half weight
against a 1000-call quota.

Historical F006 tests proved fixed-epoch admission only. Re-evaluate packing,
time precision, delayed-operation policy, quota math, CPU support or client
lifetime changes. No amd64 runtime result, contention benchmark, full workspace
test run or new network-level hot-reload test is claimed.

## Resolution references

- `link-sys-F006` — historical fixed-epoch correction, 2026-05-25.
- `retry-budget-generation-and-window-coherence` — historical investigation.
- `webhook-sliding-window-epoch-coherence` — whole-window admission correction.
- `D0019` — overlapping monotonic-time and expired-history defect.
