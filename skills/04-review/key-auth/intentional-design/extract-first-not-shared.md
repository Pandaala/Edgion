---
name: extract-first-not-shared
description: Use when reviewing findings that propose extracting a shared "first non-empty KeyGet" helper for `region_route::extract_first` and `key_auth::extract_key`; documents why the two short loops stay separate.
knowledge_type: intentional-design
---

# Separate source-selection policies, shared value reads

## Conclusion and scope

`region_route::extract_first` and `key_auth::extract_key` retain separate
first-nonempty selection loops. KeyAuth accepts only Header, Query, Cookie and
Ctx, skipping unsupported variants at runtime even if configuration validation
was bypassed. RegionRoute accepts all of its configured KeyGet variants. Do not
remove the authentication whitelist merely to share a loop.

This policy boundary does not require duplicated value parsing or owned strings.
KeyAuth now uses `runtime::session_adapter::key_value`, the unchanged borrowed
source mapping formerly local to the condition evaluator. Conditions use that
same helper. It borrows canonical session values where available and falls back
to owned KeyGet for other sources; callers keep their own eligibility, order and
empty-value policy. RegionRoute is outside this change.

## Ownership and acceptance

KeyAuth retains the first nonempty value, even when that credential subsequently
fails validation. It must not try lower-priority sources or anonymous access on
an invalid selected key. The selected source reference belongs to plugin config;
the selected value's borrow belongs to the session and ends after hashing and
lookup, before metadata writes, credential hiding or failure awaits.

Header reads preserve first-value text eligibility, Query reads the frozen
decoded cache, Cookie uses the canonical ordered parser, and Ctx reads live state.
Empty values continue to the next configured source. Key-store hashing, immutable
metadata ownership, failure delays, challenge content and sensitive logs remain.
Tests use actual headers and borrowed query/context fixtures, forbid owned reads,
and preserve existing asynchronous authentication outcome checks.

## Historical rationale and limits

The original decision cited small local loops as a reason not to consolidate
selection policy. Its claims that prefiltering necessarily allocates, generic
closures necessarily introduce indirect calls, and the former loops allocate
nothing were unsupported. Iterator filtering need not collect, generic closures
may inline, and the old KeyAuth owned getter copied selected credential values.
Those claims are not reasons to reject a bounded borrowing optimization.

No throughput result follows from the source-copy removal. Re-evaluate selection
sharing when its policy requirements converge or more consumers justify a common
contract; keep lookup mechanisms canonical independently of that choice.
The rate-limit family has its own shared key-collection policy.

## Reference Cases

- `plugins-http-F019` (2026-05-27) — closed as design tradeoff. The same
  finding also flagged the `rate_limit` / `rate_limit_redis` `collect_key_parts`
  duplication, which had already been fixed via `resolve_rate_limit_key`
  (plugins-http-F003, 2026-05-26).
- `skills/04-review/rate-limit/regression-guards/key-extraction-shared-helper.md` — the sibling
  decision for the rate-limit family, where sharing WAS the right call because
  both plugins ran byte-identical logic with no per-call filter.
