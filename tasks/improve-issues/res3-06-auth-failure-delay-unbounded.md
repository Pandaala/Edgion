# res3-06: `authFailureDelayMs` is unbounded on all nine auth plugins and holds the connection, so it is a resource amplifier rather than a brute-force control

- **Category**: best-practice
- **Severity**: P3
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/{basic_auth.rs:79, key_auth.rs:80, hmac_auth.rs:34, jwt_auth.rs:190, jwe_decrypt.rs:145, ldap_auth.rs:81, forward_auth.rs:53, header_cert_auth.rs:118, openid_connect.rs:250}` (plain `u64`, no validation)
  - `edgion-gateway/src/plugins/http/common/auth_common.rs:246-250` (`apply_auth_failure_delay`: plain `sleep`)
  - Compare `plugin_configs/mock.rs:72,102-106` and `plugin_configs/fault_injection.rs:7,145-153` (`MAX_DELAY_MS = 30_000`)

## Summary
Every HTTP auth plugin accepts `authFailureDelayMs: u64` with no upper bound. None of their
`validation_error` paths checks it. The CRD only enforces `minimum: 0`. On each failed
authentication the Gateway runs `tokio::time::sleep(delay)` while it holds the downstream connection
and request task. Other request-path sleeps in the same crate are capped at 30 s (Mock,
FaultInjection), but this one is not. A unit mistake (seconds vs. milliseconds, for example
`300000`) turns every bad credential into a five-minute connection hold.

## Current behavior (evidence)
- `apply_auth_failure_delay(delay_ms)` sleeps when `delay_ms > 0` (`auth_common.rs:246-250`). It has
  no clamp and no jitter.
- No `detect_validation_error`/`validation_error` of the nine configs reads
  `auth_failure_delay_ms`. For example, `BasicAuthConfig::detect_validation_error`
  (`basic_auth.rs:117-153`) and `KeyAuthConfig::validate_and_check` (`key_auth.rs:175-238`).
- `MockConfig::validate` rejects `delay > 30_000` (`mock.rs:102-106`).
  `DelayConfig::validate` rejects `fixedMs/maxMs > 30_000` (`fault_injection.rs:145-153`).

## Industry comparison
- NGINX `auth_delay` (1.17.10+) is the closest equivalent. It is meant for small values, and the docs
  frame it as timing-attack mitigation, not throttling.
- Kong, Envoy and APISIX do not slow brute force with per-request sleeps. They pair auth with rate
  limiting (Kong `rate-limiting` keyed on consumer/IP, Envoy `local_ratelimit`), so the attacker pays
  the cost instead of the proxy.

## Impact
A fixed sleep does not slow a parallel attacker, who just opens more connections. It does consume
Gateway connection slots, file descriptors and per-request state for the full delay. With a large
value, unauthenticated clients can tie up Gateway capacity with garbage credentials. This behaves
like a slowloris whose duration the operator chose.

## Suggested direction
- Validate `authFailureDelayMs <= 5000` (or reuse a shared `MAX_AUTH_FAILURE_DELAY_MS`) in every auth
  config's validation path, mirroring `MockConfig`/`FaultInjection`. Add `maximum` to the CRD.
- Optionally add small jitter in the shared helper. The mandatory-design page
  `shared-apply-auth-failure-delay.md` already expects jitter to live there.
- Document that brute-force protection should use `RateLimitLocal`/`RateLimitRedis` keyed on
  `RemoteIp` with `run` conditions on auth failure. The delay only blunts timing analysis.

## Notes
- `skills/04-review/auth-timing/mandatory-design/shared-apply-auth-failure-delay.md` requires all
  plugins to call the shared helper. That is satisfied, and this finding keeps the single helper.
  The page also says the helper "later adds jitter ... or metrics", so bounding and jitter fit the
  documented direction.
- Not covered by `gw-plugin-04` (rate-limit window semantics).

## Verification (independent)

Verdict: CONFIRMED. Severity P3 kept.
- `apply_auth_failure_delay` (`edgion-gateway/src/plugins/http/common/auth_common.rs:246-250`) sleeps for the raw `u64` with no clamp.
- `auth_failure_delay_ms` is a plain `u64` field in all nine configs and is never read by any validation path (grep over `edgion-resources/src`); Mock and FaultInjection cap their sleeps at 30 s.
- `shared-apply-auth-failure-delay.md` mandates the single helper and anticipates jitter there, so bounding in the helper/validation fits the documented direction. Low-effort guard against an operator unit mistake; not a default-config issue (default 0).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `apply_auth_failure_delay` (`auth_common.rs:246-250`) still sleeps the raw `u64`; the `auth_common.rs` diff only splits claim-header projection.
- `auth_failure_delay_ms` is still a plain unvalidated `u64` (e.g. `jwt_auth.rs:190`, `openid_connect.rs:250`); `auth-timing` review pages are unchanged.
