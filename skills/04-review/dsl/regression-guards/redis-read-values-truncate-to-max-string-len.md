---
name: redis-read-values-truncate-to-max-string-len
description: Use when a reviewer flags `redis.get` / `redis.hget` / `redis.hgetall` returning externally-controlled Redis strings that bypass `max_string_len` — or proposes raising `RuntimeError::StringTooLong` for them. Covers why the cap is implemented as silent truncation and which suggested fixes are not accepted.
status: fixed
finding_id: fr-dsl-01
closed: 2026-05-31
knowledge_type: regression-guards
---

# `redis.get` / `redis.hget` / `redis.hgetall` values: silent truncation at `max_string_len` — RAISING IS NOT ACCEPTED

**Status**: Resolved (fr-dsl-01, 2026-05-31) — `Vm::call_builtin_redis` for `BuiltinId::RedisGet` / `RedisHGet` / `RedisHGetAll` in `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`. The shared helper `truncate_to_max_string_len` (defined next to `checked_value_str`) is applied to each value-construction site.
**Tests**: `builtins.rs` inline `mod truncate_tests` (locks under-limit pass-through, over-limit byte truncation, and UTF-8 char-boundary walk-back). The redis client path itself requires a live `RedisLinkClient` and is covered by integration scenarios, not unit tests.

## Conclusion

The three redis read builtins (`redis.get`, `redis.hget`, and — for both key and value — `redis.hgetall`) construct `Value::Str` from server-returned strings. Each string is now **silently truncated** at `self.limits.max_string_len` (default 8 KiB) before entering the DSL value heap, restoring the sandbox invariant "no `Value::Str` in the VM heap exceeds `max_string_len`".

Findings of the form "`redis.get`/`hget`/`hgetall` skips `check_string_len`" / "a 512 MB Redis value flows unchecked into `req.set_header`/`ctx.set`/`log`" / "sandbox string-length invariant has a redis carve-out" describe behavior that existed before this fix. Fix suggestions of the form "use `check_string_len(...)?` and raise `RuntimeError::StringTooLong`" (the finding's primary suggestion) and "raising is preferable for `hgetall`" are **not accepted** — see the rationale below.

## Core Rationale

**1. Why the values need a `max_string_len` cap at all**

Essentially every other externally-controlled string builtin in the same file (`req.header`/`query`/`cookie`, `json_get`, base64/hex/url decode, regex output, hash hex) routes through a string-length guard before constructing `Value::Str`. The three redis read arms were the sole carve-out. Without the cap, an oversized Redis value (up to 512 MB; `fred` applies no reply-size limit) flows verbatim into downstream sinks (`req.set_header`, `resp.set_header`, `ctx.set`, `log`, `log_error`) that `.into_string()` with no second size gate.

**2. Why silent truncation and not `RuntimeError::StringTooLong`**

The redis dispatcher carries the documented contract "Never panics, never raises a runtime error" (`builtins.rs` doc comment on `call_builtin_redis`). Every redis failure surfaces as a returned `redis_err` **Map value** (`ok: false` + `error: Str`), which the script author handles inline. A `check_string_len(...)?` would instead propagate a `RuntimeError` up through `execute_safe` into the plugin error policy — a different, contract-breaking error surface. Silent truncation is the retained Redis-specific compatibility behavior. It must not be generalized to `http.call`, whose response overflow is required to fail the bounded exchange.

**3. Why truncation does not weaken security**

The memory peak is unchanged: the bytes are already materialized in the `fred` reply buffer before reaching the heap. Truncation only changes *what is retained* as a long-lived `Value::Str`, not *what is read* during the call — so it is not a live OOM mitigation, it is a sandbox-invariant restoration. Downstream sinks now receive at most `max_string_len` bytes, matching every other builtin. Operators who legitimately need larger values must raise `max_string_len` (explicit at config time).

**4. `hgetall` key and value both truncated**

For `hgetall`, the `max_map_size` guard (dsl-rhai-07) bounds entry *count* only. Each entry's key `k` and value `v` are independently truncated. Partial truncation does not produce a "misleading map" in a way that justifies raising: the truncated map is consistent with how every other builtin treats oversized strings, and raising would break the dispatcher contract for the whole call.

## Fix Suggestions Not Accepted

- **"Use `check_string_len(&v, max_string_len)?` to raise `StringTooLong`."** Breaks the redis dispatcher's documented "never raises a runtime error" contract; turns an oversized Redis value into a deny / 500 via the plugin error policy instead of a returned Map. Not accepted; truncation preserves both the contract and the invariant. (This is the finding's *primary* suggestion and is the one rejected.)
- **"Raising is preferable for `hgetall` because partial truncation produces a misleading map."** Same contract-break as above; dropped for the same reason. The truncated map is consistent with the rest of the module.
- **"Leave the values uncapped and document the redis carve-out."** Re-introduces the original fr-dsl-01 defect (a single carve-out in the sandbox-wide invariant). Not accepted.
- **"Add a separate `redisMaxValueLen` config knob."** The global `max_string_len` is already the right knob; an operator who needs larger values widens it. Not accepted as a narrow, low-severity finding.

## Re-evaluation Triggers

Re-open this decision only if:

- The redis dispatcher "never raises a runtime error" contract is changed for a separate reason (e.g. a global builtin error-policy redesign). At that point Redis truncation-vs-map-failure can be reconsidered.
- A production incident is traced to silent truncation hiding Redis data the operator needed; the fix would be to raise `max_string_len`, but if recurring, surface the truncation via the access log or a metric.

## Reference Cases

- `tasks/bug/fr-dsl-01-redis-string-values-bypass-max-string-len.md` — original finding (Security/P2, downgraded to P3 on Opus review), closed by this entry.
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` — the `truncate_to_max_string_len` helper and the three redis arms that call it.
- `docs/en/.../dsl/api-reference.md` and `docs/zh-CN/.../dsl/api-reference.md` — `### redis.get` / `### redis.hget` / `### redis.hgetall` truncation notes.
- See also `[[http-call-body-truncate-to-max-string-len]]` — `http.call` now fails on bounded-response overflow; its historical truncation rationale is superseded and is not Redis precedent.
- See also `[[string-len-bytes-vs-chars-contract]]` (FB-04) — why `max_string_len` is byte-measured, which is why the truncation walks back to a UTF-8 char boundary.

## Historical resolution evidence: dsl-rhai-07

`redis.hgetall` `Ok(map)` checks `map.len() > self.limits.max_map_size` before
`IndexMap` construction. The corrected public failure is the fixed
`result_too_large` class (or the generic `e` when the string budget is smaller),
not a dynamic size/limit message. See
[`redis-error-results-bounded-and-redacted.md`](redis-error-results-bounded-and-redacted.md).

## Resolution references

- `dsl-rhai-07` — 2026-05-16.
