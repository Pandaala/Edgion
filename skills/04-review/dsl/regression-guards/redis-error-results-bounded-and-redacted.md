---
name: redis-error-results-bounded-and-redacted
description: Use when reviewing DSL redis.* failure results, public error classes, maxStringLen enforcement, or dependency-detail logging.
knowledge_type: regression-guards
---

# DSL Redis error results are bounded and redact dependency detail

## Conclusion and scope

Every `redis.*` builtin keeps the non-throwing
`Map { ok: false, value: nil, error: Str }` failure contract. The script-visible
`error` is selected from a fixed public vocabulary and never includes a LinkSys
alias, Redis key or field, stored value, dependency address, or dependency error.
The value always fits `VmLimits::max_string_len`: when the configured limit cannot
hold the specific class, the generic one-byte class `e` is returned instead of a
truncated prefix. The defensive zero limit, which normal config validation rejects,
produces an empty string while retaining the same result type.

## Rationale and evidence

The canonical authority is `skills/00-design-rules.md` sections A, B, C, F, U,
and Z, plus `skills/03-coding/01-log-safety.md` and
`skills/03-coding/dsl/redis-builtin.md`. The Redis dispatcher is a Gateway request
hot path, so it must not use tracing and must not return dependency detail to the
script.

The corrected boundary is `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`:
`redis_err` accepts only the closed `RedisFailure` enum and the current string budget.
`resolve_redis_client_with` maps lookup failures to `unknown_link` or
`provider_not_ready`; every command error maps to `dependency_error`; invalid
arguments, timeouts, unknown policies, and oversized maps use their fixed classes.
`Vm::execute` passes the request's existing `PluginLog` into the async Redis
dispatcher. Dynamic dependency detail can therefore use the bounded sensitive-log
API without entering the returned `Value`.

## Alternatives and limits

Truncating a dynamic error was rejected because even a short prefix can expose a
credential, address, alias, key, or field. Propagating `StringTooLong` was also
rejected because it changes the established script-controlled failure policy into
`DslErrorPolicy`. Raising the configuration minimum to the longest class would make
small but otherwise valid sandboxes unusable. The fixed generic class preserves the
Map contract at every supported positive string limit.

This guard does not change Redis command semantics, LinkSys client construction,
EVALSHA fallback, rate-limit algorithms, or successful Redis value handling. It
does not claim that every redis-rs error contains credentials; it prevents unreviewed
dependency text from becoming a public script value regardless of its contents.

## Verification and re-evaluation

Focused tests cover unknown and not-ready links, oversized caller-controlled aliases,
the one-byte fallback, rate-limit failure envelopes under a tiny string budget, and
the dependency-detail adapter. On 2026-09-12, the DSL library selection passed 403
tests, the focused error-helper selection passed 2 tests, and independent review
confirmed all 7 Redis builtin tests and returned LGTM. Workspace format, all-target
check and Clippy passed; Clippy retained unrelated repository warnings. Agent-doc,
SSA-force, metrics-inventory, and Pingora-source guards passed. Full-workspace tests
and live integration tests were not run.

Re-evaluate if the Redis result Map is replaced, `maxStringLen` no longer permits
small positive values, PluginLog sensitive-data semantics change, or Redis failures
move to a different canonical diagnostic surface.

## Resolution references

- `bound-and-redact-dsl-redis-error-results` — confirmed security defect fixed in
  `Vm::call_builtin_redis`; source task:
  `tasks/todo/bound-and-redact-dsl-redis-error-results.md`.
