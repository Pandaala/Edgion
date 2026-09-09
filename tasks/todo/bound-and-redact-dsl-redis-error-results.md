# Bound and Redact DSL Redis Error Results

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Keep the existing non-throwing DSL Redis result map while preventing dependency details and caller-controlled key or field text from bypassing `max_string_len` through its `error` value.

## Requirements

- Inventory every `redis.*` builtin failure arm and the complete `RedisLinkClient` error chain it exposes.
- Preserve the established result family: `{ok: false, value: nil, error: <string>}`. Do not introduce another failure-policy shape or convert dependency failures into propagated runtime errors.
- Replace dependency-derived and caller-controlled error strings with fixed, documented result tags or another bounded public vocabulary.
- Ensure every script-visible Redis error string fits `VmLimits::max_string_len`, including unusually small configured limits, without silently exposing a partial sensitive value.
- Keep actionable internal detail only in the canonical bounded and redacted plugin/access-log diagnostic path; do not add request-path tracing, duplicate logs, or sensitive key/value material.
- Preserve LinkSys `RedisLinkClient`, shared rate-limiter execution, EVALSHA fallback, per-builtin success values, and caller-selected fail-open/fail-close script behavior.
- Add deterministic tests covering missing/not-ready links, each dependency-error family, caller-controlled oversized aliases/keys/fields, small string limits, and absence of sentinel sensitive detail.

## Scope

### In scope

- DSL Redis failure-result construction, safe diagnostic propagation, directly required Redis wrapper error shaping, documentation, and focused unit tests.

### Out of scope

- Redis command semantics, new failure-policy controls, cache/client construction, rate-limit algorithms, or unrelated LinkSys consumers.

## Acceptance criteria

- No `redis.*` failure returns a script-visible string longer than `max_string_len` or containing dependency addresses, traces, credentials, keys, fields, or values.
- Existing scripts can continue branching on `ok`, reading `value`, and treating `error` as a string.
- Internal diagnostics are bounded, redacted, and owned by the canonical plugin/access-log surface.
- Focused DSL/Redis tests and all repository guards pass.
