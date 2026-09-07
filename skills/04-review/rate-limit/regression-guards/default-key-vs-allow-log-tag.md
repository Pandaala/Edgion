---
name: default-key-vs-allow-log-tag
description: Use when reviewing rate-limit key-miss PluginLog tag findings; covers why the default_key fallback and OnKeyMiss::Allow paths now use distinct access-log tags.
status: fixed
finding_id: m8-01
closed: 2026-05-30
knowledge_type: regression-guards
---

# Rate-limit `default_key` fallback vs `OnKeyMiss::Allow` share a log tag — FIXED (m8-01)

## Conclusion

`resolve_rate_limit_key` (`edgion-gateway/src/plugins/http/common/mod.rs`) used to push
the identical `"key miss:allow"` PluginLog tag for two semantically opposite key-miss
outcomes: the `default_key` fallback (request **is** rate-limited under the shared
default-key bucket — `RateLimitKey::Resolved`) and `OnKeyMiss::Allow` (request
**bypasses** rate-limiting entirely — `RateLimitKey::Allow`). The `default_key` path
now pushes `"key miss:default"`; the `OnKeyMiss::Allow` path keeps `"key miss:allow"`.

The three key-miss outcomes are now distinguishable from the access log alone:

| Path | Tag |
|---|---|
| `default_key` present, key not found (rate-limited under shared key) | `"key miss:default"` |
| `OnKeyMiss::Allow`, key not found (full bypass) | `"key miss:allow"` |
| `OnKeyMiss::Deny`, key not found (rejected) | `"Deny key miss"` |

Re-reports of "default_key path logs `key miss:allow` despite being rate-limited" are
not accepted — verify the current `default_key` arm pushes `"key miss:default"` before
re-reporting.

## Core Rationale

**1. Single shared call site, caller-side enum mapping unchanged.**

Both `RateLimit::run_request` and `RateLimitRedis::run_request` map
`RateLimitKey::Resolved(k)` to the quota check and `RateLimitKey::Allow` to
`GoodNext`. Only the helper's `plugin_log.push` literal changed; the enum contract
and both callers are untouched. This is purely an observability fix — no execution or
enforcement behavior changed.

**2. The tag follows the access-log wording rule.**

`"key miss:default"` is event+action joined by a colon, all-lowercase, ≤4 words,
within the 15–40 byte budget — consistent with `skills/03-coding/observability/00-access-log.md`
rule 4. No central tag registry exists, so no registration step is needed; the
access-log doc's `"key miss:allow"` example remains valid for the Allow path.

**3. Distinct from plugins-http-F003.**

`plugins-http-F003` extracted `resolve_rate_limit_key` and unified the
`"Deny key miss"` denial tag, but did not touch the `default_key` tag. This finding
(m8-01) is the follow-up that disambiguates the two key-miss-*allow-shaped* outcomes.

## Fix Suggestions Not Accepted

- Keep a single tag and document the ambiguity instead — defeats the purpose; an
  operator reading the access log cannot retroactively know which route had
  `default_key` configured.
- Add a `tracing::*` log to distinguish the paths — data-plane hot path, forbidden by
  the zero-tracing rule; the access-log tag is the correct observability surface.

## Re-evaluation Triggers

Re-open this decision only if:

- The rate-limit key-resolution outcomes are restructured (e.g. `RateLimitKey` gains a
  new variant) such that the three-tag mapping no longer covers every key-miss path.

## Reference Cases

- m8-01 (2026-05-30) — `tasks/bug/m8-01-rate-limit-default-key-misleading-log-tag.md`
- Source: `edgion-gateway/src/plugins/http/common/mod.rs` (`resolve_rate_limit_key`)
- Related: `skills/04-review/rate-limit/regression-guards/key-extraction-shared-helper.md` (plugins-http-F003)
- Wording rule: `skills/03-coding/observability/00-access-log.md` §"PluginLog entry wording rules"
