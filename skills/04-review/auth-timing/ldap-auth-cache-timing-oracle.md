---
name: ldap-auth-cache-timing-oracle
description: Use when reviewing ldap_auth cache-hit-vs-miss timing-side-channel findings; explains why the fast cache hit path is an accepted tradeoff and why no jitter / username-only-key fix is applied.
---

# LDAP Auth Cache-Hit vs Cache-Miss Timing Difference — ACCEPTED-RISK

## Conclusion

The response-time difference between the `ldap_auth` cache-hit path (`check_cache` → local SHA-256 lookup, sub-millisecond) and the cache-miss path (`ldap_bind` → network round-trip, multiple milliseconds) is **known, evaluated, and accepted as a design tradeoff** documented in the plugin's Security Recommendations. It **should not** require adding response jitter, a username-only cache key, or any other code change on the success path.

Fix suggestions of the form "add jitter / random sleep to the cache-hit response", "cache only the username (not the password) to remove the oracle", or "make cache hit and cache miss take constant time" are **not accepted**.

## Core Rationale

**1. The timing oracle reveals strictly less than the HTTP status code already does**

A cache hit requires the attacker to submit the *exact* valid `(username, password)` pair (the key is `SHA-256(username:password)`). Whether cached or not, a valid credential ultimately returns `200` and an invalid one returns `401` — credential **validity is already disclosed by the response status**. The only additional bit the timing leaks is *cache state*: whether that exact pair was authenticated recently (within `cacheTtl`). That signal is strictly weaker and presupposes the attacker already holds the full credential, so it does not advance an attacker who is enumerating credentials. This is why Round B correctly rated it Medium (a cache-state oracle, not a credential oracle).

**2. The proposed code mitigations are anti-patterns or unsafe**

- Adding jitter / a sleep to the cache-hit path penalises **every legitimate authenticated request** on the data-plane hot path — the same class of objection that rejects dummy-hash verify in `basic_auth` (see [basic-auth-username-enumeration.md](basic-auth-username-enumeration.md)). Caching auth results and returning fast on a hit is the universal gateway practice (Nginx auth caching, Envoy, Kong); none add hit-path jitter.
- "Cache only the username (not the password)" is dangerous if read as the cache **key**: the key *must* bind the password, otherwise any password for a known username would hit and bypass authentication. The cache *value* is already the username; the key already hashes `username:password`. The suggestion either is a no-op (value already username-only) or, applied to the key, is an auth-bypass.

**3. The failure-side delay is already uniform; remaining mitigation is config + control-plane**

`apply_auth_failure_delay(auth_failure_delay_ms)` is already applied on all failure paths (no-credentials, invalid header, bad password — `plugin.rs` lines 256, 261, 294), so failure timing is not an enumeration oracle. Operators who care about the residual cache-state signal can lower `cacheTtl` (or set it to `0` to disable caching) and stack the `RateLimit` plugin; this is documented in the plugin's Security Recommendations.

## Fix Suggestions Not Accepted

- Add jitter / random sleep to the cache-hit response — adds latency to every legitimate authenticated request on the data-plane hot path; gateway anti-pattern.
- Cache only the username and not the password — the cache value is already the username; removing the password from the *key* is an authentication bypass.
- Make cache hit and cache miss constant-time — would require artificially slowing every cache hit to LDAP-bind latency, defeating the entire point of the cache.
- Apply `auth_failure_delay_ms` to the cache-miss success path — the delay is a failure-path brute-force mitigation; the success path already does not branch on validity in a way the status code does not already expose.

## Re-evaluation Triggers

Re-open this decision only if:

- A deployment requires hiding *cache membership* itself as a compliance hard requirement (then disable caching via `cacheTtl: 0` rather than adding hit-path jitter).
- The cache key scheme changes such that a hit no longer requires the full `(username, password)` pair.

## Reference Cases

- F-012 (Round B confirmed Medium, downgraded to ⚠️ accepted tradeoff 2026-05-21).
- Sibling decision: [basic-auth-username-enumeration.md](basic-auth-username-enumeration.md) (same "no hit-path code fix" reasoning for basic_auth username-existence timing).
- Cross-feature rules: [../security/iron-rules.md](../security/iron-rules.md) (timing-attack reachability + network-jitter checks).
- Source: `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs` (`check_cache` / `cache_key` / `apply_auth_failure_delay` call sites).
- Doc: `docs/{en,zh-CN}/user-guide/edgion-plugins/ldap-auth.md` Security Recommendations.
