# Basic Auth Username-Existence Timing Difference: No Code-Layer Dummy Bcrypt

## Conclusion

The response-time difference between the "username does not exist" and "username exists but password wrong" paths in the `basic_auth` plugin (<1ms vs ~200ms bcrypt) is **known, evaluated, and mitigated via documentation**. It **should not** require introducing dummy bcrypt or a similar code fix.

## Reasons

- **Dummy bcrypt is an anti-pattern in gateway scenarios**: forcing bcrypt for unknown users (~200ms / 1 core) creates a CPU-amplification DoS vector; an attacker can burn the gateway's compute with enumeration traffic
- **Peer gateway products consistently do not implement it**: Traefik (`basicauth`), Nginx (`auth_basic`), Envoy (`basic_auth` filter), Kong (`basic-auth` plugin), HAProxy (`http-request auth`), APISIX (`basic-auth`) all do not perform code-layer dummy verify; this is "the mainstream gateway-layer practice"
- **Spring Security / Django doing dummy verify** is because in framework-layer single-machine validation scenarios there is no CPU-amplification concern, which is not equivalent to the gateway scenario
- **Edgion's mitigation is at the config layer**: `BasicAuthConfig.authFailureDelayMs` exists and is available; the `RateLimit` plugin can stack on top to limit enumeration rate

## Decision Rule

When a review/audit report contains the following patterns, close as "no code fix":

1. "basic_auth returns directly when username does not exist; bcrypt not executed" / "username enumeration timing side channel"
2. "Recommend dummy bcrypt::verify on a fixed hash to eliminate timing differences"
3. "Should set `auth_failure_delay_ms` default to 500ms" (a breaking default change; not auto-applied)

Verify three points before closing:

- `basic_auth/plugin.rs` does not introduce dummy bcrypt (keeps the "return immediately for unknown user" path)
- `BasicAuthConfig.auth_failure_delay_ms` still defaults to `0` (operator enables explicitly)
- `docs/{zh-CN,en}/user-guide/http-route/filters/edgion-plugins/basic-auth.md` "Security Best Practices" section still contains a "Defense against enumeration and brute force (recommended config)" paragraph, with recommended value `>= 500`

All three satisfied → not a real issue; point to the M4 FIXED (doc-only) decision in `tasks/security-audit/01-auth-plugins.md` and close.

## Background Notes

- Decision date: 2026-04-21
- Decision source: `tasks/security-audit/` M4 entry, chose Option C
- Related finding: H8 (admin login username comparison is not CT) in this audit belongs to the admin API path rather than the plugin path; this conclusion does not apply; needs separate evaluation
