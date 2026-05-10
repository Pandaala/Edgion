---
name: fully-review
description: Edgion global code review guide. Includes the structured workflow for AI-driven full-codebase reviews, per-dimension checklists, and confirmed false positives / closed items (skip directly on the next review).
---

# Edgion Global Review Guide

> **When to use**: Use this guide when performing systematic full-codebase reviews (security, correctness, observability, etc.) on the entire codebase, or when assessing AI- or externally-submitted review reports and judging false positives. For day-to-day single-file or single-feature reviews, use `04-review/SKILL.md`.

> **Use case**: Performing global security / quality reviews on the Edgion codebase, or evaluating newly submitted review reports.
> **Order of execution**: Read this file first → load each dimension on demand → cross-check against closed items to skip known false positives.

---

## Workflow

```
1. Load existing review knowledge (04-review/)
2. Cross-check the closed-items list and skip known false positives
3. Inspect each dimension below in turn
4. For each finding: verify the exploitation path first, then assign severity
```

---

## Dimension 1: Security

### General principle

Before flagging any security issue, you must answer:

1. **Is the attack path reachable?** What prerequisites does the attacker need?
2. **Is there independent entropy that blocks the attack?** (See the Timing Attack rules below.)
3. **Does the fix address the root cause?** The root issue with weak password formats is the algorithm being weak, not the comparison method.

### Timing attack rules

Do not classify "uses non-constant-time comparison" as high severity by itself. You must verify:

| Check | Description |
|-------|-------------|
| Whether the compared value contains independent random entropy | If a CSPRNG nonce is included and verification requires all fields to match, a timing oracle cannot be exploited |
| Whether network jitter swamps the timing difference | In gateway scenarios, millisecond-level jitter is far larger than nanosecond-level short-circuit differences; measured signal-to-noise ratio is extremely low |
| Whether the attacker can control the input and retry at scale | Not exploitable when there is rate limiting or single-use token consumption |

**Typical false positive**: signature comparison for CSRF tokens. The token contains a CSPRNG random (256-bit); even if the sign can be guessed, it cannot be paired with the unknown random, so timing attacks cannot exploit it.

### Weak password formats

The root cause of weak hashes (SHA-1, MD5) is the **lack of key-stretching**; adding constant-time comparison is treating the symptom. Correct handling:

- The real threat for weak formats is offline brute-force, not timing side channels
- Constant-time fixes provide very little benefit for already-weak hashes; priority should be lowered
- Correct action: where stronger schemes are needed, point operators to an external auth (`ForwardAuth` / `ExtAuthz`) — the gateway data plane intentionally does not embed bcrypt-class verification (would force a result cache); emit weak-format warnings in the control plane (when the Controller loads config), not on the data-plane hot path

### Password / key cache keys

Memory-leak risk of using plaintext or reversibly-encoded values (base64) as LRU cache keys:

- Must be analyzed together with the **cache value**: if the value is the username, a cache-key collision causes authentication to be attributed to the wrong user — more severe than a memory leak
- Hashed-key schemes (SHA-256 / ahash) need to guarantee no collisions, otherwise they introduce auth bypass
- CRC32 collision probability is unacceptable at thousand-user scale and is unsuitable as an authentication cache key
- If memory safety is not a compliance hard requirement, this issue can be downgraded to an observation item

### Upstream address safety

When reviewing paths that rewrite or select upstreams, load [04-review/ssrf/upstream-address-safety.md](../04-review/ssrf/upstream-address-safety.md). Key distinctions:

- `domainMap` / `backendRefs` / RegionRoute PM are authorization boundaries, not equivalent to final dial-address safety.
- Production upstream final dial addresses reject loopback and link-local / metadata by default; in the Endpoint-derived path, only integration testing mode may allow loopback for local fixtures.
- RFC1918 private networks are legitimate K8s / cross-cluster / internal-API targets and must not be blanket-blocked.

---

## Dimension 2: Correctness

- Controller resource handling requeue rules → [04-review/handler-requeue/](../04-review/handler-requeue/handler-requeue-rule.md)
- "Memory leak" judgment → [04-review/memory-leak/](../04-review/memory-leak/not-a-memory-leak.md)
- Route architecture consistency → [04-review/architecture/route-architecture-consistency.md](../04-review/architecture/route-architecture-consistency.md)
- LB counters: increment/decrement symmetry of LeastConn counters across retries

---

## Dimension 3: Observability

Full rules → [04-review/observability/](../04-review/observability/observability-review-rule.md)

Quick checks:

- [ ] No `tracing::*` on the data-plane hot path
- [ ] PluginLog ≤ 40 bytes, records only the final decision, contains no sensitive values
- [ ] No Histogram metrics; label cardinality is bounded
- [ ] `ctx_map` contains no raw JWT claims / passwords

---

## Dimension 4: Architectural layering

- Weak password detection and format warnings → **Controller layer** (during config load), not on the data plane
- Configuration validation and schema legality → Controller
- Request routing, plugin execution, authentication → Gateway data plane
- If the data plane needs operational warnings, express them via PluginLog → access log; do not use `tracing::warn!`

---

## Closed items (skip directly during review)

The table below records items confirmed not to need fixing after deep analysis. If the same issue is encountered again during review, refer to this conclusion and close it directly.

| ID | Module | Description | Conclusion | Reason for closure |
|----|--------|-------------|------------|--------------------|
| P0-04 | `csrf/token.rs` | `self.sign != expected_sign` is non-constant-time comparison; `SHA-256(expires+random+key)` is not HMAC | **Closed** | The token contains a 256-bit CSPRNG random; a timing oracle cannot pair with the random for exploitation; length extension has no exploitable path because validation is exact-match |
| P0-04b | `csrf/token.rs` | Key mixing uses SHA-256 instead of HMAC | **Closed** | Same as above; verification is exact-match, and length-extension-constructed variants cannot pass `gen_sign` format validation |
| P0-09 | `csrf/token.rs` | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` on the hot path (DoS risk) | **Closed** | The trigger requires the system clock to be earlier than 1970, which is unreachable in practice (NTP step-back / VM drift will not roll back 56 years; `SystemTime` returns UTC and is not affected by time zone). **The proposed fix `unwrap_or(Duration::from_secs(0))` actually has a security side effect**: during verify, `now=0`, and `0 - expires` (positive) is negative, always less than max_expires, so the expiration check is silently bypassed and CSRF protection fails. Panicking exposes the issue, which is safer than silently degrading security. |
