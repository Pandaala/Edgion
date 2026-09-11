---
name: basic-auth-username-enumeration
description: Use when reviewing basic_auth timing-side-channel or username-enumeration findings; explains why no dummy-hash verify is added and how the risk is mitigated via documentation.
knowledge_type: regression-guards
---

# Basic Auth Username-Existence Timing Difference: No Code-Layer Dummy Verify

## Conclusion

The response-time difference between the "username does not exist" and "username exists but password wrong" paths in the `basic_auth` plugin is **known, evaluated, and mitigated via documentation**. It **should not** require introducing a dummy hash verify or a similar code fix.

After bcrypt was removed from the plugin (the data plane now only verifies `{SHA}` / `$apr1$` / `$1$`, all sub-microsecond), the absolute time difference is much smaller, but the user-existence signal is still observable on a LAN, and is enough to drive offline brute-force against any leaked hash. The reasoning below still applies.

The discriminating signal is **not only** the hash-compute time. The known-user path runs `tokio::task::spawn_blocking(verify).await` (`plugin.rs:127`) — blocking-pool scheduling + thread handoff + `.await` round-trip — while the unknown-user path returns synchronously at the `HashMap` miss (`plugin.rs:122`). That dispatch overhead is present for every known user and absent for every unknown one, so even the cheapest `{SHA}` hash leaves a measurable delta. This refinement does **not** change the conclusion: the rejected fix would have to wrap the dummy verify in the *same* `spawn_blocking` to be equalizing, which schedules a blocking-pool task for every bogus-username probe — strictly *more* CPU/thread-pool amplification under enumeration traffic than the hash-only objection, and a uniform `authFailureDelayMs` floor masks both the compute-time and the dispatch-overhead source at once.

## Reasons

- **Dummy verify is an anti-pattern in gateway scenarios**: even with cheap hashes, running an extra verify for unknown users amplifies wasted CPU under enumeration traffic; with bcrypt-class hashes it is even worse (~200ms / 1 core per probe)
- **Peer gateway products consistently do not implement it**: Traefik (`basicauth`), Nginx (`auth_basic`), Envoy (`basic_auth` filter), Kong (`basic-auth` plugin), HAProxy (`http-request auth`), APISIX (`basic-auth`) all do not perform code-layer dummy verify; this is "the mainstream gateway-layer practice"
- **Spring Security / Django doing dummy verify** is because in framework-layer single-machine validation scenarios there is no CPU-amplification concern, which is not equivalent to the gateway scenario
- **Edgion's mitigation is at the config layer**: `BasicAuthConfig.authFailureDelayMs` exists and is available; the `RateLimitLocal` plugin can stack on top to limit enumeration rate

## Decision Rule

When a review/audit report contains the following patterns, close as "no code fix":

1. "basic_auth returns directly when username does not exist; verify not executed" / "username enumeration timing side channel"
2. "Recommend dummy hash verify on a fixed hash to eliminate timing differences"
3. "Should set `auth_failure_delay_ms` default to 500ms" (a breaking default change; not auto-applied)
4. "The discriminating signal is the `spawn_blocking` dispatch overhead (thread handoff / scheduling), not just hash-compute time" — same finding, same conclusion (see Conclusion section); the equalizing dummy verify would have to go through the same `spawn_blocking`, amplifying blocking-pool work per probe.

Verify three points before closing:

- `basic_auth/plugin.rs` does not introduce a dummy verify (keeps the "return immediately for unknown user" path)
- `BasicAuthConfig.auth_failure_delay_ms` still defaults to `0` (operator enables explicitly)
- `docs/{zh-CN,en}/user-guide/http-route/filters/edgion-plugins/basic-auth.md` "Security Best Practices" section still contains a "Defense against enumeration and brute force (recommended config)" paragraph, with recommended value `>= 500`

All three satisfied → not a real issue; point to the M4 FIXED (doc-only) decision in `tasks/security-audit/01-auth-plugins.md` and close.

## Background Notes

- Decision date: 2026-04-21 (original); refreshed when bcrypt was removed from the data plane; refreshed again 2026-05-26 for the F-005 re-report (spawn_blocking dispatch-overhead framing — same conclusion)
- Decision source: `tasks/security-audit/` M4 entry, chose Option C
- Related finding: H8 (admin login username comparison is not CT) in this audit belongs to the admin API path rather than the plugin path; this conclusion does not apply; needs separate evaluation
