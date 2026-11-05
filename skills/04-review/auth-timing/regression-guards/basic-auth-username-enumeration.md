---
name: basic-auth-username-enumeration
description: Use when reviewing basic_auth timing-side-channel or username-enumeration findings; explains why no dummy-hash verify is added and how the risk is mitigated via documentation.
knowledge_type: regression-guards
---

# Basic Auth Username-Existence Timing Difference: No Code-Layer Dummy Verify

## Conclusion

The response-time difference between the "username does not exist" and "username exists but password wrong" paths in the `basic_auth` plugin is **known, evaluated, and mitigated via documentation**. It **should not** require introducing a dummy hash verify or a similar code fix.

After bcrypt was removed from the plugin (the data plane now only verifies `{SHA}`, well under a microsecond for typical passwords, and `$apr1$` / `$1$`, 1000 MD5 rounds costing roughly 0.1-0.7 ms for passwords up to the 256-byte cap; measured 2026-11-05 in a release build at about 0.05 us for a short SHA password and 125 us / 670 us for 16- / 256-byte MD5-crypt passwords), the absolute time difference is much smaller, but the user-existence signal is still observable on a LAN, and is enough to drive offline brute-force against any leaked hash. The reasoning below still applies.

Historical note (superseded 2026-11-05): the known-user path used to run `tokio::task::spawn_blocking(verify).await`, a leftover from the bcrypt era, so a blocking-pool handoff added a dispatch-overhead signal on top of the hash time. Verification now runs inline (`BasicAuth::verify_user`), so the only remaining discriminator is the hash-compute time itself. The conclusion is unchanged: an equalizing dummy verify would still burn the full hash cost (about 0.1-0.7 ms for an MD5-class entry) for every bogus-username probe. The remaining delta is accepted, not masked: `authFailureDelayMs` is a fixed sleep added after verification (`apply_auth_failure_delay`), so total failure time is verify time plus the delay and the sub-millisecond known-user difference stays measurable by averaging. The delay slows guessing; rate limiting bounds how many probes can be averaged. Re-evaluate if the delay is changed to a floor measured from request start.

## Reasons

- **Dummy verify is an anti-pattern in gateway scenarios**: even with cheap hashes, running an extra verify for unknown users amplifies wasted CPU under enumeration traffic; with bcrypt-class hashes it is even worse (~200ms / 1 core per probe)
- **Peer gateway products consistently do not implement it**: Traefik (`basicauth`), Nginx (`auth_basic`), Envoy (`basic_auth` filter), Kong (`basic-auth` plugin), HAProxy (`http-request auth`), APISIX (`basic-auth`) all do not perform code-layer dummy verify; this is "the mainstream gateway-layer practice"
- **Spring Security / Django doing dummy verify** is because in framework-layer single-machine validation scenarios there is no CPU-amplification concern, which is not equivalent to the gateway scenario
- **Edgion's mitigation is at the config layer**: `BasicAuthConfig.authFailureDelayMs` slows each failed guess (it does not equalize timing); the `RateLimitLocal` plugin can stack on top to limit enumeration rate

## Decision Rule

When a review/audit report contains the following patterns, close as "no code fix":

1. "basic_auth returns directly when username does not exist; verify not executed" / "username enumeration timing side channel"
2. "Recommend dummy hash verify on a fixed hash to eliminate timing differences"
3. "Should set `auth_failure_delay_ms` default to 500ms" (a breaking default change; not auto-applied)
4. "The discriminating signal is the `spawn_blocking` dispatch overhead (thread handoff / scheduling)" — stale: verification no longer uses `spawn_blocking` (see the historical note in the Conclusion section); the hash-time conclusion applies.

Verify three points before closing:

- `basic_auth/plugin.rs` does not introduce a dummy verify (keeps the "return immediately for unknown user" path)
- `BasicAuthConfig.auth_failure_delay_ms` still defaults to `0` (operator enables explicitly)
- `docs/{zh-CN,en}/user-guide/http-route/filters/edgion-plugins/basic-auth.md` "Security Best Practices" section still contains a "Defense against enumeration and brute force (recommended config)" paragraph, with recommended value `>= 500`

All three satisfied → not a real issue; point to the M4 FIXED (doc-only) decision in `tasks/security-audit/01-auth-plugins.md` and close.

## Background Notes

- Decision date: 2026-04-21 (original); refreshed when bcrypt was removed from the data plane; refreshed again 2026-05-26 for the F-005 re-report (spawn_blocking dispatch-overhead framing — same conclusion); refreshed 2026-11-05 when verification moved inline and the MD5-class cost was measured; same day, corrected the earlier claim that `authFailureDelayMs` masks the timing delta (it is added after verify and does not)
- Decision source: `tasks/security-audit/` M4 entry, chose Option C
- Related finding: H8 (admin login username comparison is not CT) in this audit belongs to the admin API path rather than the plugin path; this conclusion does not apply; needs separate evaluation
