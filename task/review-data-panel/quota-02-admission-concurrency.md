# Quota admission concurrency

Status: deferred optimization for user decision. No runtime change.
Source baseline: 18ec43cae.

## Evidence

RedisQuotaSink::admit_all in edgion-gateway/src/ai/quota/runtime.rs awaits each
configured target in order. Denied results accumulate the maximum retry_after
and continue; Redis errors or unexpected statuses return immediately. The caller
wraps the whole sequence in one request_timeout. One-shot throttle may perform
one more admission within its remaining action/request budget.

The shared cumulative_quota_admission operation uses the canonical Redis executor.
Lua operation 0 reads the aggregate with HGET and returns without creating or
extending quota keys. AiTokenQuotaConfig validation in
edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs permits
1 through 8 limits. Distinct target keys do not provide an atomic multi-key
snapshot, whether read sequentially or concurrently.

Bounded concurrent reads could reduce waiting for several targets, but no latency
or pool-saturation improvement has been measured. They change read observation
times, commands issued after an earlier target would have failed, and which
operations complete inside the existing deadline. Concurrent settlement in
RedisQuotaSink::apply_all is a separate contract, not evidence that concurrent
admission is behavior-preserving. The worker-future representation proposal in
quota-01 also does not authorize this scheduling change.

## Decision

Choose whether to retain serial admission or evaluate bounded concurrency with
explicit acceptance of its observation, load and failure-timing differences.
This is not currently eligible for an impact-free optimization. Read-only Redis
operations do not make changed scheduling or increased failure-path work free.

## Acceptance for a selected implementation

- Define aggregate outcomes for all-admitted, one/multiple denials, errors and
  unexpected statuses. Preserve error-over-denial precedence and maximum denied
  retry_after unless a separate policy change is explicitly approved.
- Define whether all targets are started eagerly and which local futures are
  cancelled on failure or timeout. Cancellation must not claim to recall Redis
  commands already sent, or create detached work or application-level replay.
- Keep one total admission timeout, the one-shot throttle budget, request deadline
  checks, and configured Redis fail-open/fail-closed behavior. Do not reset a
  per-target timeout to extend the total admission budget.
- Bound concurrent work to the validated target inventory, currently at most 8.
  Measure one versus several targets, low/high pool contention, early/late errors,
  denials and hanging operations. Report command counts and pool pressure as well
  as latency; do not infer throughput from mocked immediate futures.
- Use the canonical LinkSys client and shared executor, opaque keys and Redis
  Cluster routing. Keep exact NOSCRIPT-only fallback; uncertainty is not a reason
  to replay a potentially completed command. No multi-key snapshot guarantee is
  introduced.
- Add deterministic scheduling/outcome tests and isolated Redis validation for
  the chosen implementation. Existing quota and shared-core tests alone do not
  prove parallel admission semantics. Keep settlement concurrency, accumulator
  ownership and response-body behavior outside this change.
- Use a new IMPL plan with an exact implementation allow-list before edits.

## Verification limits

The current audit passed 27 quota and 29 shared Redis network-free tests,
formatting, workspace check/Clippy and agent-document/diff validation. These
verify the unchanged baseline, not the proposed concurrency or real Redis timing.
Relevant original knowledge: shared Redis executor unification and
skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md.
