---
name: retry-backoff-minimum-wait-conformance
description: Use when reviewing retry/backoff findings; covers why a tight-deadline retry whose remaining budget is smaller than the configured backoff is failed (not retried immediately) to honour the GEP-1731 minimum-wait clause.
status: fixed
finding_id: route-06
closed: 2026-06-12
---

# route-06 — Retry backoff minimum wait can be skipped under a tight deadline — FIXED

## Conclusion

When the remaining `request` / `grpc-timeout` budget is smaller than the
computed exponential `backoff` delay (`0 < deadline_remaining <= delay`), the
gateway now **fails the request** with the binding deadline's 504 timeout
instead of issuing an immediate sub-minimum retry. This honours GEP-1731's
clause that "unsuccessful backend requests are not retried before the configured
minimum duration".

Re-reports of the form "tight-deadline retry skips the configured minimum
backoff / retry fires immediately when `remaining < backoff` / GEP-1731
minimum-wait violated" are **closed by this fix** — verify the `match
deadline_remaining` arms in `upstream_peer` before re-reporting. Conversely,
proposals to revert to the old "skip the sleep and retry immediately under a
tight deadline" behaviour are **not accepted** (it was the non-conformant path).

## Core Rationale

**1. GEP-1731 permits backoff only with a respected minimum wait**

The spec allows exponential/capped/jittered backoff "as long as unsuccessful
backend requests are not retried before the configured minimum duration". The
old code computed `should_sleep = delay < remaining` and, when `remaining <=
delay`, skipped the sleep entirely; the post-sleep re-check only aborted when a
deadline had *actually expired*, so in the `0 < remaining <= delay` window the
retry proceeded immediately — before the configured minimum backoff. Option 1
(conformant) was chosen with operator input over Option 2 (status quo +
disclosure).

**2. The sacrificed behaviour is low-value**

An immediate retry under a sub-backoff budget is a single extra attempt with a
very narrow success window (remaining budget is, by definition, smaller than one
backoff interval) and is exactly what the GEP minimum-wait clause discourages.
Failing fast with the binding 504 timeout is cheaper and spec-correct; operators
who want the final attempt to run size `timeouts.request` to leave room for a
full `backoff` interval (documented in `retry.md`).

**3. Implementation**

`upstream_peer` (`edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`)
replaces the `should_sleep` bool with a `match deadline_remaining`:
- `Some(r) if delay < r` → sleep the full backoff (budget to spare).
- `None` → no request/grpc deadline applies, sleep the full backoff.
- `Some(_)` (i.e. `remaining <= delay`) → add the binding error
  (`RequestTimeoutExceeded`, preferred to mirror the post-sleep re-check order,
  else `GrpcClientDeadlineExceeded`), stamp the upstream entry 504, and return a
  504 `PingoraError`.

The post-sleep deadline re-check is retained as a defensive guard against timer
jitter overshooting the sleep.

## Fix Suggestions Not Accepted

- Revert to "skip the sleep and retry immediately when `remaining <= delay`" — that is the non-conformant path this finding fixed.
- Clamp the sleep to the remaining budget and retry anyway — still retries before the configured minimum duration, so it also violates the GEP minimum-wait clause.
- Keep the docs disclosure note framing this as a known non-conformance — obsolete; the note now describes the conformant fail-fast behaviour.

## Re-evaluation Triggers

Re-open only if:

- Upstream Gateway API relaxes or removes the GEP-1731 minimum-wait clause.
- Operators demonstrate a need to attempt one immediate retry under a
  sub-backoff budget (would revive Option 2 as an opt-in knob, not a default).

## Reference Cases

- Origin finding: `tasks/todo/route-06-backoff-minimum-wait-conformance.md` (per-try-timeout deep review, GEP conformance reviewer), closed 2026-06-12.
- Related: [per-try-timeout-via-backend-request.md](per-try-timeout-via-backend-request.md) (route-02, the per-try timeout mechanism this backoff path serves).
- Docs: `docs/en/user-guide/http-route/resilience/retry.md` (and zh-CN) — the disclosure note was replaced with the conformant-behaviour note.
