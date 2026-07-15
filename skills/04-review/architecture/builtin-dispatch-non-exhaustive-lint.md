---
name: builtin-dispatch-non-exhaustive-lint
description: Use when reviewing DSL `BuiltinId` dispatch findings that propose enabling `#![warn(non_exhaustive_omitted_patterns)]` on the gateway runtime dispatcher or the edgion-dsl compiler table, OR that report the Redis-builtin predicate as duplicated, OR that propose converting `BuiltinId::is_redis_builtin` to an exhaustive `match`.
---

# `non_exhaustive_omitted_patterns` Lint on `BuiltinId` Dispatch — NOT-AN-ISSUE

## Conclusion

Adding `#![warn(non_exhaustive_omitted_patterns)]` (or `#![deny(...)]`) does
not strengthen the current contract. `BuiltinId` is an exhaustive public enum
defined in `edgion-dsl`; normal Rust exhaustiveness checking applies both to
compiler matches in that crate and runtime matches in `edgion-gateway`.
Marking the enum `#[non_exhaustive]` merely to activate the lint in the consumer
crate would weaken the stronger compile-time contract by requiring a wildcard.
The protective work is therefore exhaustive enum matches without `_ =>`
catch-alls. The string-pair namespace resolver cannot be exhaustive and remains
test-protected.

Fix suggestions of the form "add `#![warn(non_exhaustive_omitted_patterns)]` to make `BuiltinId` dispatch fail compilation on new variants" are **not accepted**.

## Core Rationale

**1. The `non_exhaustive_omitted_patterns` lint is scoped to external `#[non_exhaustive]` types**

Per the Rust reference, the lint detects a wildcard that omits known variants of
an external `#[non_exhaustive]` type. `BuiltinId` is not
`#[non_exhaustive]`. It lives in `edgion-dsl/src/bytecode.rs`; compiler tables
are in the same crate, while the runtime dispatcher is in `edgion-gateway`.
Neither side benefits from this lint under the current enum contract.

**2. Native exhaustiveness checking is the real lever**

For same-crate enum matches, removing the `_ =>` arm makes rustc emit E0004 (`non_exhaustive_patterns`) the moment a new variant is added — strictly stronger than a warning. The current `BuiltinId` dispatch surface has four sites:

| Site | File:line | Status |
|------|-----------|--------|
| `call_builtin` | `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` | Exhaustive `BuiltinId` match |
| `call_builtin_redis` | same runtime file | Exhaustive `BuiltinId` match with explicit non-Redis variants |
| `builtin_expected_argc` | `edgion-dsl/src/compiler/builtins.rs` | Exhaustive `BuiltinId` match |
| `resolve_namespace_method` | same compiler file | `match (&str, &str)`, structurally cannot be exhaustive; `_ => None` is required |

For `resolve_namespace_method` the match key is a `(&str, &str)` tuple with an infinite domain; no lint or rustc check can force "every BuiltinId has a name mapping". A name-table omission must be caught by tests, not by the type system. The dispatch-side compile-time guarantee is the strongest enforcement available.

**3. The placebo cost matters**

Landing `#![warn(non_exhaustive_omitted_patterns)]` on these files would create a future trap: a reviewer adds a new `BuiltinId` variant, sees no warning, assumes dispatch is wired, and ships. The runtime then surfaces an `Internal` error, which `DslErrorPolicy::Deny` converts to a 500 with the variant name lost. Removing the false sense of protection is itself a small reliability win.

## Fix Suggestions Not Accepted

- "Add `#![warn(non_exhaustive_omitted_patterns)]` (or `#![deny(...)]`) to the runtime or compiler table" — the enum is not `#[non_exhaustive]`; native exhaustive matches are stronger
- "Mark `BuiltinId` as `#[non_exhaustive]` and enable the lint" — this forces an external wildcard and weakens the current compile-time break on new variants
- "Switch to nightly `#![feature(non_exhaustive_omitted_patterns_lint)]`" — feature was stabilized; the limitation is the cross-crate scope, not stability
- "Add an exhaustiveness-check unit test that pattern-matches every variant" — duplicates what native E0004 already does for the three enum-matching sites; adds no coverage for `resolve_namespace_method` (string match, see Rationale 2)

## Re-evaluation Triggers

Re-open this decision only if:

- Rust adds a same-crate-aware variant of `non_exhaustive_omitted_patterns` (or a separate lint such as a hypothetical `unreachable_exhaustive_wildcard`) that does fire on internal enums with `_ =>` arms.
- A new `BuiltinId` dispatch site is added that legitimately needs a `_ =>` catch-all (e.g., a generic fallback dispatcher), at which point the trade-off becomes specific to that site.
- The public bytecode compatibility policy changes and intentionally marks
  `BuiltinId` as `#[non_exhaustive]`; only then reconsider the consumer-side
  lint while preserving explicit known-variant coverage.

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

- Historical `dsl-vm-rhai-research` issue 14 — closed via the explicit-list rewrite of `call_builtin_redis`; the task files are no longer present
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs::Vm::call_builtin_redis` — reference implementation of explicit non-Redis variant listing
- `edgion-dsl/src/compiler/builtins.rs::builtin_expected_argc` — compiler-side exhaustive table
