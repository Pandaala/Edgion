---
name: oidc-callback-path-precomputed
description: Use when reviewing findings that report OpenID Connect callback-path parsing or normalization on every request; the path is derived once when the plugin is constructed.
knowledge_type: regression-guards
---

# OIDC callback path is precomputed — FIXED (gw-plugin-oidc-01)

## Conclusion

`OpenidConnect` stores its normalized callback path when the plugin instance is
constructed. `callback_path()` only borrows that cached `String`, so the request
filter and callback-cookie paths do not parse `redirectUri`, read the global path
normalization configuration, or allocate a normalized path per request.

This is safe because `redirectUri` is immutable for the plugin instance and the
path normalization configuration is process-lifetime state initialized through a
`OnceLock` before resource watches start. Route and plugin compilation may read
the global configuration during the first snapshot, before listeners serve
requests. Initializing it after `start_watch_kinds` races with lazy default
initialization, causing a debug startup panic or retaining the wrong policy in
release builds. Plugin configuration changes construct a
new instance and therefore a new cached callback path.

## Review rule

Re-reports of per-request OIDC callback-path parsing or normalization are stale if
`OpenidConnect::new` initializes `callback_path` and the accessor returns `&str`.
Re-open only if callback-path derivation gains request-dependent input or the path
normalization configuration becomes hot-reloadable independently of plugin
reconstruction.

## Verification

The existing unit tests cover both path-only and absolute-URL `redirectUri`
values, including path normalization.


## Resolution references

- `gw-plugin-oidc-01` — 2026-08-02.
