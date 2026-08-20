---
name: extract-first-not-shared
description: Use when reviewing findings that propose extracting a shared "first non-empty KeyGet" helper for `region_route::extract_first` and `key_auth::extract_key`; documents why the two short loops stay separate.
---

# `extract_first` / `extract_key` duplication — DESIGN-TRADEOFF (not consolidated)

## Conclusion

The two "iterate `&[KeyGet]`, call `session.key_get`, return the first non-empty
value" loops in `region_route::extract_first`
(`edgion-gateway/src/plugins/http/region_route/plugin.rs:127`) and
`key_auth::extract_key`
(`edgion-gateway/src/plugins/http/key_auth/plugin.rs:84`) are intentionally NOT
consolidated into a `common/` helper. Re-reports of the form "extract a shared
`first_key_get` helper" are not accepted on their own.

This is distinct from the rate-limit `collect_key_parts` pattern, which IS shared
via `common::resolve_rate_limit_key` (see
`skills/04-review/rate-limit/key-extraction-shared-helper.md`).

## Core Rationale

**1. The two loops are not behavior-identical.**

`key_auth::extract_key` filters the `KeyGet` variant before calling
`session.key_get` — only `Header`, `Query`, `Cookie`, `Ctx`, `Webhook` are
honored; `RemoteIp`, `Path`, `Method`, etc. are silently skipped. This is a
runtime defense-in-depth in addition to whatever the resource validator enforces.
`region_route::extract_first` honors every `KeyGet` variant.

A shared helper would have to either (a) pre-filter the slice (one allocation
per request on the hot path), (b) take a `Fn(&KeyGet) -> bool` predicate
parameter (extra closure + indirection for marginal payoff), or (c) drop the
runtime filter from `key_auth` (loses defense-in-depth). None of these is a net
win for a 14-line / 21-line pair.

**2. The duplication cost is bounded.**

Each loop is short (14 and 21 lines including signature and braces), local to a
single plugin file, and exercises a stable public API (`PluginSession::key_get`).
A behavior change to `key_get` semantics would already require touching every
caller; the two extract-loops do not add meaningful surface area to that change.

**3. Hot-path code prefers low-abstraction shapes.**

Both functions run on every request these plugins process. The current shape is
a straight `for` loop with no closures, no allocations, no `dyn` indirection
beyond the existing `&dyn PluginSession`. A shared helper with a predicate
closure would add a layer of indirection for no behavioral gain.

## Fix Suggestions Not Accepted

- Add a `pub async fn first_key_get(session, sources) -> Option<(String, &KeyGet)>`
  in `common/` and call it from both plugins — `key_auth` cannot use it without
  losing its variant filter or paying the per-request filtering cost above.
- Pass a predicate closure to a shared helper — adds indirection on the hot path
  for no observable behavior change; the two call sites would each need a
  distinct closure anyway.
- Move `extract_first` onto `PluginSession` as a default method — bloats the
  trait surface for two callers; same predicate-vs-filter tradeoff applies.

## Re-evaluation Triggers

Re-open this decision only if:

- A third plugin grows the same "first non-empty `KeyGet`" loop, raising the
  duplication count from 2 to 3+.
- `KeyGet` semantics or `PluginSession::key_get` signature changes in a way
  that requires nontrivial logic at every call site (e.g., async result type
  changes, error propagation added).
- `key_auth` drops its variant filter (e.g., the resource validator becomes the
  single source of truth) — at that point the two loops become byte-identical
  and the case for sharing strengthens.

## Reference Cases

- `plugins-http-F019` (2026-05-27) — closed as design tradeoff. The same
  finding also flagged the `rate_limit` / `rate_limit_redis` `collect_key_parts`
  duplication, which had already been fixed via `resolve_rate_limit_key`
  (plugins-http-F003, 2026-05-26).
- `skills/04-review/rate-limit/key-extraction-shared-helper.md` — the sibling
  decision for the rate-limit family, where sharing WAS the right call because
  both plugins ran byte-identical logic with no per-call filter.
