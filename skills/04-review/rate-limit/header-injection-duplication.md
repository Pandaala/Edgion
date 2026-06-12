---
name: header-injection-duplication
description: Decision rule for findings about duplicated rate-limit response-header injection across the RateLimit and RateLimitRedis plugins, or proposals to extract a shared write_rate_limit_headers / write_rate_limit_rejection_headers helper. Read this file first when reviewing those findings.
---

# Rate-limit response-header injection duplication — ACCEPTED TRADEOFF (plugins-http-F004, 2026-05-26)

## Conclusion

The structurally parallel `add_headers` / `reject_request` header-injection blocks in
`rate_limit/plugin.rs` (`104-202`) and `rate_limit_redis/plugin.rs` (`372-467`) are
**deliberately NOT extracted** into a shared helper. The duplication is an accepted
maintenance tradeoff because the two emitters diverge by design at every axis a shared
helper would have to unify.

Fix suggestions of the form "extract `write_rate_limit_headers(session, ..)` /
`write_rate_limit_rejection_headers(resp, ..)` into `plugins/http/common/`" are
**not accepted**.

## Core Rationale

**1. The two emitters diverge by design, not by accident.**

- **Reset on the allow path**: `rate_limit` emits `X-RateLimit-Reset`; `rate_limit_redis`
  intentionally omits it — the Lua scripts return only `{allowed, current, remaining,
  retry_ms}`, so there is no precise window-reset timestamp for an allowed request.
- **`Retry-After` emission contract**: both plugins now share the same RFC 7231 §7.1.3
  guard — omit the header on `Duration::ZERO` (`OnMissingKey::Deny`, and on the Redis side
  `OnRedisFailure::Deny`) and ceil any positive sub-second value to 1 s (g4-005 for Redis,
  plugins-http-FB01 for local). This axis no longer differs; the remaining divergences
  below are still enough to defeat a shared helper.
- **Header gating on the reject path**: `rate_limit_redis::reject_request` only writes
  limit/remaining/reset when a `result: Option<&RateLimitResult>` is present; `rate_limit`
  always has its values.
- **Signatures / data sources differ**: `rate_limit` takes `(remaining: isize,
  reset_after: Duration)` and computes `limit` from config; `rate_limit_redis` takes
  `(remaining: i64, limit: u64)` straight from the script result.

**2. The write target differs between the two code paths.**

The allow path writes via `session.set_response_header(name, value)`; the reject path
writes via `resp.insert_header(name, value)` on a `ResponseHeader` being built. A single
helper would need a write-target abstraction (a closure or two separate helpers) layered
on top of the value divergence above — added indirection, not removed duplication.

**3. Coupling two intentionally-independent emitters is the wrong tradeoff.**

"Duplication is far cheaper than the wrong abstraction." A shared helper that serves both
must accrete conditional parameters (`Option` reset, rounding mode, `result` gating) until
it is no simpler than the inline code, while every future edit to one plugin's headers now
has to reason about the other. For two emitters that are meant to evolve independently,
"change one without touching the other" is a correct property, not a defect.

**4. The genuinely-shared part is already shared.**

Header-*name* resolution (custom names vs default `X-RateLimit-*`) already goes through
`config.get_header_names()` / `headers.limit_header()` etc. on the config type. What
remains duplicated is thin per-plugin emission glue bound to each plugin's policy.

**5. Precedent F-003 drew the line exactly here.**

The sibling key-extraction finding *was* extracted (`resolve_rate_limit_key`,
[key-extraction-shared-helper.md](key-extraction-shared-helper.md)) because its output was
genuinely identical and it fixed a real log-tag drift. But that record explicitly kept the
`reject_request` mapping caller-side *because the two reject signatures differ*. F-004 asks
to dive into the `reject_request` / `add_headers` bodies and unify that same divergent
surface — crossing the line F-003 set. Same redundancy class as link-sys-F008 (collapse
divergent `match` arms into one generic — not accepted).

## Fix Suggestions Not Accepted

- Extract `write_rate_limit_headers` / `write_rate_limit_rejection_headers` shared helpers —
  couples two intentionally-divergent emitters and needs a write-target + value-divergence
  parameterization that does not reduce net complexity.
- Re-extract the `Retry-After` guard into a shared helper now that both plugins follow the
  same RFC 7231 §7.1.3 rule (g4-005 + plugins-http-FB01) — the per-plugin two-line
  `if !retry_after.is_zero() { ceil-and-insert }` body is short, inline, and lives inside
  otherwise-divergent `reject_request` bodies; a freestanding helper does not shrink the
  surface enough to justify the extra indirection.
- Add the reset header to the Redis allow path "for consistency" — the Lua script returns
  no window-reset timestamp; emitting a fabricated one would be incorrect.

## Re-evaluation Triggers

Re-open this decision only if:

- A third rate-limit plugin variant is added that needs the same emission (a shared helper
  then amortizes across 3+ callers and the abstraction earns its keep).
- A new header naming standard (e.g. RFC 9440 `RateLimit-*`) must be applied uniformly AND
  the change lands in the per-plugin emission glue rather than the config-level name
  accessors that already centralize naming.

## Reference Cases

- plugins-http-F004 (2026-05-26) — this finding, accepted tradeoff.
- plugins-http-F003 — sibling key-extraction (extracted): [key-extraction-shared-helper.md](key-extraction-shared-helper.md).
- g4-005 — Redis `Retry-After` rounding / `ZERO` omission decision.
- plugins-http-FB01 (2026-05-27) — local `rate_limit::reject_request` gained the same
  `Duration::ZERO` omit + sub-second ceil guard, eliminating the RFC 7231 §7.1.3
  divergence on `OnMissingKey::Deny`.
- link-sys-F008 — same redundancy class, "collapse divergent arms" declined.
- Source: `edgion-gateway/src/plugins/http/rate_limit/plugin.rs:104-202`,
  `edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs:372-467`.
</content>
</invoke>
