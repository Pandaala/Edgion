---
name: review-dsl
description: Review knowledge for dsl, grouped by decision type.
---

# dsl review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Intentional design

- [`non_exhaustive_omitted_patterns` Lint on `BuiltinId` Dispatch — NOT-AN-ISSUE](intentional-design/builtin-dispatch-non-exhaustive-lint.md)
- [DSL compiler `Compiler::leave_scope` assert-panic (plugins-http-dsl-F006)](intentional-design/compiler-leave-scope-assert.md)
- [EdgionDSL custom VM vs Rhai / mlua / rune / Boa — DESIGN-DECISION (LOCKED)](intentional-design/dsl-design-decisions.md)
- [DSL plugin validate-then-construct double `resolve_script` — DESIGN-TRADEOFF (ACCEPTED)](intentional-design/dsl-validate-construct-double-resolve.md)
- [`call_builtin` Giant-Match Per-Arm Extract-Fn — DESIGN-TRADEOFF](intentional-design/giant-match-extract-fn-declined.md)
- [`http.call` bounded-response failures: status-zero result rather than RuntimeError](intentional-design/http-call-body-truncate-to-max-string-len.md)
- [DSL `Value::Map` is intentionally Boxed — do not unbox](intentional-design/map-variant-is-boxed.md)
- [`check_redis_link_names` Positional Heuristic — FIXED (fail-open), PIN-IN-PLACE](intentional-design/redis-link-check-positional-heuristic.md)
- [DSL `Value::Str` SSO / try_unwrap migration — investigated and deferred](intentional-design/sso-migration-not-worth-it.md)
- [DSL `len()` (chars) vs `max_string_len` (bytes) — DIVERGENCE IS INTENTIONAL, CONTRACT NOW EXPLICIT](intentional-design/string-len-bytes-vs-chars-contract.md)
- [DSL `url_decode` builtin uses form-encoding semantics intentionally — DESIGN-TRADEOFF](intentional-design/url-decode-form-semantics.md)

## Regression guards

- [DSL Redis rate-limit policy contract](regression-guards/redis-rate-limit-policy-contract.md)

- [Chrono `Display::to_string()` Panic on Bad Format — FIXED, PIN-IN-PLACE](regression-guards/chrono-display-panic.md)
- [DSL regex constants: DFA size_limit / dfa_size_limit are capped at 1MB](regression-guards/dfa-size-limit.md)
- [Digest output string limit](regression-guards/digest-output-string-limit.md)
- [DSL `http.call` response aggregation is bounded while streaming](regression-guards/http-call-full-body-read-before-truncate.md)
- [`redis.get` / `redis.hget` / `redis.hgetall` values: silent truncation at `max_string_len` — RAISING IS NOT ACCEPTED](regression-guards/redis-read-values-truncate-to-max-string-len.md)
