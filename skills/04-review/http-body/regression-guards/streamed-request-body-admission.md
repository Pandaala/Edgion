---
name: streamed-request-body-admission
description: Use when reviewing streamed request cap accounting, trailer rejection, capture composition, or ordered mirror publication.
knowledge_type: regression-guards
---

# Streamed request admission and publication

## Conclusion and scope

The common request-body hook owns independent cumulative original-input and
final-output physical-cap accounts for monotonic handler-selected requests.
Input admission precedes all processors; output admission follows their ordered
verdicts and precedes WAF, observers, RequestMirror and upstream publication.
Equality is admitted; overflow/excess terminates with generic 413 regardless of
processor fail-open policy. Actual request trailers terminate this path with
400 before synthetic clean EOS. Ordinary and WAF-only trailer behavior is outside
this handler-specific rule.

## Rationale and evidence

`RequestBodyPluginState` owns numeric accounts without adding state to ordinary
requests. `pg_request_body_filter_action` is the single policy boundary; handlers
retain one current event and transport-bounded verdict. Cleanup abandons incomplete
handlers and mirror work. ExtProc marks its request direction done on successful
Complete or terminal failure, so cleanup cannot rewrite an initiating failure or
completed result into cancellation. Already selected downstream responses are
not replaced.

Request EOS may leave future response phases pending on the same Process stream.
Abandonment closes that retained owner immediately, preserving any established
terminal result instead of deferring resource release to logging.

Complete-body cache/pre-read followed by STREAMED can bypass trailer detection
and let a mirror send a stale detached snapshot. `PingoraSessionAdapter` rejects
this combination in either order, including bodyless replay-owner creation.
Protection is runtime across scopes; no static whole-chain validation is claimed.

## Alternatives and limits

Do not add per-plugin cap enforcement, a second body cache, a speculative event
queue, or synthetic processor EOS after gateway rejection. Intermediate processor
output remains bounded by the gRPC message limit; the physical final-output cap
is applied after the complete chain. A valid prefix may have reached upstream
before a later cap/trailer rejection; rejection cancels without certifying a
complete upload and does not roll back upstream application effects.

## Verification and re-evaluation

The hook unit tests exercise independent cumulative caps, fail-open retention,
ordered backpressure, final mirror bytes, terminal chain cleanup and EOS outcomes.
Session adapter tests cover both complete-capture composition orders and bodyless
mutation refusal. ExtProc integration checks two noncommutative byte transforms
and exact mirror/backend parity over H1/H2; Transport checks trailer rejection in
all four downstream/upstream protocol pairs. Fresh command outcomes are recorded
in the task implementation record after verification.

Re-evaluate if complete-body/streamed composition is designed explicitly, the
trailer transport contract changes, or processor run-ahead is introduced.
