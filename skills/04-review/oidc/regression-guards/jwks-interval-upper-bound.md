---
name: oidc-jwks-interval-upper-bound
description: Use when reviewing OpenID Connect JWKS cache deadline arithmetic, `Instant + Duration` overflow in `JwksState::publish_fetch_result`, or bounds on `jwksCacheTtl` / `jwksMinRefreshInterval`.
status: fixed
finding_id: auth-03
closed: 2026-11-10
knowledge_type: regression-guards
---

# OIDC JWKS Interval Fields Are Bounded at Admission — FIXED

## Conclusion and scope

`OpenidConnectConfig::validation_error` (`edgion-resources`) accepts
`jwksCacheTtl` and `jwksMinRefreshInterval` only in `1..=MAX_JWKS_INTERVAL_SECS`
(31536000 seconds, one year). The Gateway JWKS provider
(`edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs`,
`JwksState::publish_fetch_result`) keeps the plain `completed_at + ttl` and
`completed_at + min_refresh` additions; their safety rests on this bound.

## Rationale and evidence

`std::time::Instant + Duration` panics on overflow. Before the fix both fields
were `u64` seconds checked only for non-zero, so a value such as `u64::MAX` was
admitted and every JWKS publication on the request path panicked, with the
cache never populated (a per-request panic loop).

The same `validation_error` runs on both validated paths, so a bound there
covers every production construction:

- Controller admission through `EdgionPlugin::validation_error`.
- Gateway plugin-runtime build (`plugin_runtime.rs`): enabled entries are
  checked by `plugin_validation_error` before construction; an invalid entry
  is never passed to `OpenidConnect::try_new` (the only production
  construction site) and the runtime is sealed fail-closed.

`OpenidConnect::try_new`'s own `config.get_validation_error()` check only
gates discovery prewarm; it does not block construction.

## Alternatives and limits

- Envoy `jwt_authn` takes the same admission-only shape (upstream `main`,
  read 2026-11-10): `RemoteJwks.cache_duration` is proto-bounded
  (`api/envoy/extensions/filters/http/jwt_authn/v3/config.proto`) and
  `source/extensions/filters/http/jwt_authn/jwks_cache.cc` re-validates twice
  the seconds value so the later `now + duration` cannot overflow; the runtime
  addition is unchecked.
- A runtime `checked_add` was considered and not added: no validated path
  reaches the addition with an unbounded value. One year is far below any
  platform `Instant` limit, and no legitimate JWKS cache or refresh interval
  approaches it (defaults are 300 s and 10 s).
- Limit: direct construction bypasses validation: unit tests build
  `JwksProviderConfig` directly, and the test-only `OpenidConnect::new` or a
  direct `try_new` call does not reject an invalid config. Such callers must
  keep values small.

## Verification and re-evaluation

`edgion-resources` unit test `test_validation_bounds_jwks_intervals` covers 1,
the maximum, maximum + 1, `u64::MAX` and 0 for both fields (passing on
2026-11-10).

Reconsider if either field becomes a duration string, if the bound is raised
near platform `Instant` limits, or if a new code path builds
`JwksProviderConfig` from operator input without `validation_error`.

## Resolution references

- auth-03 (Gateway panic audit, 2026-11-08): OIDC JWKS cache deadline used
  unchecked `Instant + Duration` with unbounded operator TTLs. Fixed by the
  admission bound on 2026-11-10.
