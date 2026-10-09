---
name: key-extraction-shared-helper
description: Decision rule for findings about duplicated rate-limit key extraction across the RateLimitLocal and RateLimitRedis plugins, divergent key-miss log tags, or proposals to change the shared helper's return type. Read this file first when reviewing those findings.
knowledge_type: regression-guards
---

# Rate-limit key extraction shared helper — FIXED (plugins-http-F003, 2026-05-26)

## Conclusion

The key-extraction block that resolves a rate-limit key from `config.key` sources
was copy-pasted between `rate_limit/plugin.rs` and `rate_limit_redis/plugin.rs`,
and had already silently diverged (`OnKeyMiss::Deny` logged `"Deny key miss"`
in one and `"Deny rate limit"` in the other). It is now extracted into a single
shared free function:

```rust
// edgion-gateway/src/plugins/http/common/mod.rs
pub enum RateLimitKey { Resolved(String), Allow, Deny }

pub fn resolve_rate_limit_key(
    session: &mut dyn PluginSession,
    keys: &[KeyGet],
    default_key: Option<&str>,
    on_missing: &OnKeyMiss,
    plugin_log: &mut PluginLog,
) -> RateLimitKey
```

Both plugins call it; the canonical key-miss-denial tag is `"Deny key miss"`
(distinct from a quota-exceeded denial). Re-reports of "key extraction is
duplicated" or "`Deny` log tags diverge between the two plugins" are not accepted —
verify the current call sites still route through `resolve_rate_limit_key` before
re-reporting.

## Core Rationale

**1. The helper returns an enum, not `Result<String, PluginRunningResult>`.**

The original finding suggested `-> Result<String, PluginRunningResult>`. That is
not accepted: the `Deny` branch must call the plugin's own `reject_request`, whose
signature differs between the two plugins (`RateLimitRedis::reject_request` takes
an extra `Option<&RateLimitResult>`). A shared helper cannot produce that
`PluginRunningResult`. The `RateLimitKey` enum keeps key resolution + log tagging
in one place while leaving the `Allow → GoodNext` / `Deny → self.reject_request(..)`
mapping caller-side, where the differing reject signatures live.

**2. `keys` is `&[KeyGet]`, not `&[String]`.**

`RateLimitLocalConfig.key` and `RateLimitQuota.key` are `Vec<KeyGet>` (a key-source enum consumed by `session.key_get`),
not a list of literal strings. The finding's `&[String]` signature was inaccurate.

**3. `on_missing` is taken by reference.**

`OnKeyMiss` is not `Copy`; passing it by value would move out of the shared
`&self.config`. The helper takes `&OnKeyMiss` to keep the change scoped to the
gateway (no `Copy` derive added to the types layer).

## Fix Suggestions Not Accepted

- Change `resolve_rate_limit_key` to return `Result<String, PluginRunningResult>`
  (cannot encode the two divergent `reject_request` signatures).
- Re-report the duplicated block or the divergent `Deny` log tags (now unified).

## Resolution references

- `plugins-http-F003` — 2026-05-26.
