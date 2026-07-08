---
name: builtin-dispatch-non-exhaustive-lint
description: Use when reviewing DSL `BuiltinId` dispatch findings that propose enabling `#![warn(non_exhaustive_omitted_patterns)]` on `lang/builtins.rs` or `lang/compiler/builtins.rs` to catch forgotten arms when new `BuiltinId` variants are added, OR that report the Redis-builtin predicate (`is_redis_builtin` / `is_redis_builtin_id`) as duplicated, OR that propose converting `BuiltinId::is_redis_builtin` to an exhaustive `match`.
---

# `non_exhaustive_omitted_patterns` Lint on `BuiltinId` Dispatch — NOT-AN-ISSUE

## Conclusion

Adding `#![warn(non_exhaustive_omitted_patterns)]` (or `#![deny(...)]`) to the two `BuiltinId` dispatch files is a placebo: the lint only fires for matches over `#[non_exhaustive]` types from **other crates**. `BuiltinId` is defined in the same crate and is not marked `#[non_exhaustive]`, so the lint never triggers regardless of where the attribute is set. The protective work is done by removing `_ =>` catch-all arms instead, which forces native E0004 / `non_exhaustive_patterns` errors. Three of the four candidate matches already do this; only `call_builtin_redis` carried a `_ =>` arm, which has now been expanded to an explicit non-Redis variant list (issue-14 fix).

Fix suggestions of the form "add `#![warn(non_exhaustive_omitted_patterns)]` to make `BuiltinId` dispatch fail compilation on new variants" are **not accepted**.

## Core Rationale

**1. The `non_exhaustive_omitted_patterns` lint is scoped to external `#[non_exhaustive]` types**

Per the Rust reference, the lint detects a wildcard pattern that is reachable in a match on a `#[non_exhaustive]` enum or struct **from a different crate**. Within the defining crate `#[non_exhaustive]` is treated as exhaustive, and the lint is suppressed. `BuiltinId` lives in `edgion-dsl/src/bytecode.rs` alongside its match sites in the same `edgion` crate, so even if the enum were re-marked `#[non_exhaustive]` the lint would still not fire on these dispatch files.

**2. Native exhaustiveness checking is the real lever**

For same-crate enum matches, removing the `_ =>` arm makes rustc emit E0004 (`non_exhaustive_patterns`) the moment a new variant is added — strictly stronger than a warning. The current `BuiltinId` dispatch surface has four sites:

| Site | File:line | Status |
|------|-----------|--------|
| `call_builtin` | `lang/builtins.rs:74-…` | Already exhaustive (no `_ =>`) |
| `call_builtin_redis` | `lang/builtins.rs:996-…` | Made exhaustive (issue-14) by replacing `_ =>` with explicit non-Redis variant list |
| `builtin_expected_argc` | `lang/compiler/builtins.rs:91-…` | Already exhaustive (no `_ =>`) |
| `resolve_namespace_method` | `lang/compiler/builtins.rs:7-84` | `match (&str, &str)`, **structurally cannot be exhaustive** — `_ => None` is required |

For `resolve_namespace_method` the match key is a `(&str, &str)` tuple with an infinite domain; no lint or rustc check can force "every BuiltinId has a name mapping". A name-table omission must be caught by tests, not by the type system. The dispatch-side compile-time guarantee is the strongest enforcement available.

**3. The placebo cost matters**

Landing `#![warn(non_exhaustive_omitted_patterns)]` on these files would create a future trap: a reviewer adds a new `BuiltinId` variant, sees no warning, assumes dispatch is wired, and ships. The runtime then surfaces an `Internal` error, which `DslErrorPolicy::Deny` converts to a 500 with the variant name lost. Removing the false sense of protection is itself a small reliability win.

## Fix Suggestions Not Accepted

- "Add `#![warn(non_exhaustive_omitted_patterns)]` (or `#![deny(...)]`) to `lang/builtins.rs` / `lang/compiler/builtins.rs`" — lint never fires for same-crate non-`#[non_exhaustive]` enums; ships dead config that looks protective
- "Mark `BuiltinId` as `#[non_exhaustive]` and enable the lint" — same-crate `#[non_exhaustive]` is treated as exhaustive; still a no-op for these dispatch files
- "Switch to nightly `#![feature(non_exhaustive_omitted_patterns_lint)]`" — feature was stabilized; the limitation is the cross-crate scope, not stability
- "Add an exhaustiveness-check unit test that pattern-matches every variant" — duplicates what native E0004 already does for the three enum-matching sites; adds no coverage for `resolve_namespace_method` (string match, see Rationale 2)

## Re-evaluation Triggers

Re-open this decision only if:

- Rust adds a same-crate-aware variant of `non_exhaustive_omitted_patterns` (or a separate lint such as a hypothetical `unreachable_exhaustive_wildcard`) that does fire on internal enums with `_ =>` arms.
- A new `BuiltinId` dispatch site is added that legitimately needs a `_ =>` catch-all (e.g., a generic fallback dispatcher), at which point the trade-off becomes specific to that site.
- The `BuiltinId` enum is moved to a separate crate (e.g., a published `edgion-dsl-bytecode` crate consumed by external integrations), where `#[non_exhaustive]` + the lint would start to bite.

## Related: Redis-predicate consolidation (F-004, 2026-05-26)

The two byte-for-byte-identical private predicates that enumerated the 13 `Redis*`
variants — `is_redis_builtin` in `lang/vm.rs` (VM dispatch loop) and
`is_redis_builtin_id` in `plugin.rs` (compile-time link-name check) — have been
consolidated into a single inherent method `BuiltinId::is_redis_builtin(self) -> bool`
in `bytecode.rs`, the same module as the enum. Both call sites now invoke it, so a new
`Redis*` variant only has to be reflected in one place.

The method body stays a `matches!` (non-exhaustive) **intentionally**, not an exhaustive
`match` returning `bool`: a boolean predicate has no `_ =>` arm to remove, so making it
"exhaustive" would force every future *non-Redis* variant addition to also touch this
predicate (adding a `BuiltinId::X => false` arm) for zero protection on the Redis set it
guards. This is the same stance as the rest of this rule — compile-time safety comes from
explicit non-`_` lists at the actual dispatch matches (`call_builtin_redis`) plus tests,
not from type-system gymnastics on a side predicate.

Re-reports of the form "`is_redis_builtin` / `is_redis_builtin_id` are duplicated" are now
moot (single source). Suggestions to "convert `BuiltinId::is_redis_builtin` to an
exhaustive `match`" are **not accepted** for the reason above.

## Reference Cases

- `tasks/todo/dsl-vm-rhai-research/issues/issue-14-enable-non-exhaustive-lint.md` — closed via the explicit-list rewrite of `call_builtin_redis`
- `tasks/todo/dsl-vm-rhai-research/04-builtin-registration.md` §"Quick improvements worth doing now" — original proposal (now superseded by this rule and the issue-14 fix)
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs::call_builtin_redis` — reference implementation of explicit non-Redis variant listing (the pattern any future BuiltinId-matching dispatcher should follow)
