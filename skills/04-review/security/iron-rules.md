---
name: security-iron-rules
description: Use when reviewing authentication, timing attacks, password handling, or admin endpoints; documents the cross-feature security iron rules that apply project-wide (timing, weak passwords, cache keys).
---

# Security Iron Rules

These rules apply project-wide across all authentication and security review dimensions. Load this file before flagging timing, password, or cache-key findings.

---

## General principle

Before flagging any security issue, you must answer:

1. **Is the attack path reachable?** What prerequisites does the attacker need?
2. **Is there independent entropy that blocks the attack?** (See the Timing Attack rules below.)
3. **Does the fix address the root cause?** The root issue with weak password formats is the algorithm being weak, not the comparison method.

---

## Timing attack rules

Do not classify "uses non-constant-time comparison" as high severity by itself. You must verify:

| Check | Description |
|-------|-------------|
| Whether the compared value contains independent random entropy | If a CSPRNG nonce is included and verification requires all fields to match, a timing oracle cannot be exploited |
| Whether network jitter swamps the timing difference | In gateway scenarios, millisecond-level jitter is far larger than nanosecond-level short-circuit differences; measured signal-to-noise ratio is extremely low |
| Whether the attacker can control the input and retry at scale | Not exploitable when there is rate limiting or single-use token consumption |

**Typical false positive**: signature comparison for CSRF tokens. The token contains a CSPRNG random (256-bit); even if the sign can be guessed, it cannot be paired with the unknown random, so timing attacks cannot exploit it.

---

## Weak password formats

The root cause of weak hashes (SHA-1, MD5) is the **lack of key-stretching**; adding constant-time comparison is treating the symptom. Correct handling:

- The real threat for weak formats is offline brute-force, not timing side channels
- Constant-time fixes provide very little benefit for already-weak hashes; priority should be lowered
- Correct action: where stronger schemes are needed, point operators to an external auth (`ForwardAuth` / `ExtAuthz`) — the gateway data plane intentionally does not embed bcrypt-class verification (would force a result cache); emit weak-format warnings in the control plane (when the Controller loads config), not on the data-plane hot path

---

## Password / key cache keys

Memory-leak risk of using plaintext or reversibly-encoded values (base64) as LRU cache keys:

- Must be analyzed together with the **cache value**: if the value is the username, a cache-key collision causes authentication to be attributed to the wrong user — more severe than a memory leak
- Hashed-key schemes (SHA-256 / ahash) need to guarantee no collisions, otherwise they introduce auth bypass
- CRC32 collision probability is unacceptable at thousand-user scale and is unsuitable as an authentication cache key
- If memory safety is not a compliance hard requirement, this issue can be downgraded to an observation item

---

## Upstream address safety

When reviewing paths that rewrite or select upstreams, load [04-review/ssrf/upstream-address-safety.md](../ssrf/upstream-address-safety.md). Key distinctions:

- `domainMap` / `backendRefs` / RegionRoute PM are authorization boundaries, not equivalent to final dial-address safety.
- Production upstream final dial addresses reject loopback and link-local / metadata by default; in the Endpoint-derived path, only integration testing mode may allow loopback for local fixtures.
- RFC1918 private networks are legitimate K8s / cross-cluster / internal-API targets and must not be blanket-blocked.

---

## Historical cases

| ID | Module | Description | Conclusion |
|----|--------|-------------|------------|
| P0-04 | `csrf/token.rs` | Non-constant-time `sign` comparison | Closed — 256-bit CSPRNG random blocks timing oracle exploitation |
| P0-04b | `csrf/token.rs` | SHA-256 key mixing instead of HMAC | Closed — exact-match validation blocks length-extension exploitation |
| P0-09 | `csrf/token.rs` | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` on hot path | Closed — requires clock pre-dating 1970; `unwrap_or(0)` would silently bypass expiry check |

See [../CLOSED-FINDINGS.md](../CLOSED-FINDINGS.md) for the full cross-topic closure log.
