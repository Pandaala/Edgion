---
name: security-iron-rules
description: Use when reviewing authentication, timing attacks, password handling, or admin endpoints; documents the cross-feature security iron rules that apply project-wide (timing, weak passwords, cache keys).
knowledge_type: mandatory-design
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
| Whether the compared value contains independent random entropy | Establish which values remain unknown and cannot be selected at the verification boundary; randomness visible in a supplied token does not establish that barrier |
| Whether network jitter swamps the timing difference | Measure the relevant signal and noise under the actual query model; network jitter alone is not evidence that a timing signal cannot be extracted |
| Whether the attacker can control the input and retry at scale | Verify actual enforcement and the resulting query budget; rate limiting or single-use consumption is not blanket immunity |

**Current CSRF evidence**: `CsrfToken::decode` reads `random`, `expires` and
`sign` from client-supplied Base64 JSON after the plugin checks header/cookie
agreement. The nonce is not an unknown server secret. After lifetime checks,
`verify_at` rejects public signature lengths other than 64 bytes, computes HMAC
for the remaining inputs and compares all 64 bytes with an XOR fold. This is a
source-level property, not a machine-code constant-time proof or an exploitability
assessment. Preserve the full comparison; nonce entropy does not justify an
early-exit secret comparison. See the [signature implementation evidence](../../csrf/regression-guards/signature-input-fragments.md).

---

## Weak password formats

The root cause of weak hashes (SHA-1, MD5) is the **lack of key-stretching**; adding constant-time comparison is treating the symptom. Correct handling:

- The real threat for weak formats is offline brute-force, not timing side channels
- Constant-time fixes provide very little benefit for already-weak hashes; priority should be lowered
- Correct action: where stronger schemes are needed, point operators to an external auth (`ForwardAuth` / `ExtAuthz`) — the gateway data plane intentionally does not embed bcrypt-class verification (would force a result cache); emit weak-format warnings in the control plane (when the Controller loads config), not on the data-plane hot path

---

## Password / key cache keys

Memory-leak risk of using plaintext or reversibly-encoded values (base64) as cache keys:

- Must be analyzed together with the **cache value**: if the value is the username, a cache-key collision causes authentication to be attributed to the wrong user — more severe than a memory leak
- Hashed-key schemes (SHA-256 / ahash) need to guarantee no collisions, otherwise they introduce auth bypass
- CRC32 collision probability is unacceptable at thousand-user scale and is unsuitable as an authentication cache key
- If memory safety is not a compliance hard requirement, this issue can be downgraded to an observation item

---

## Access-log group attribution (no topology-probing oracle)

Auth plugins that support credential grouping — BasicAuth / KeyAuth / HmacAuth
(`secret_groups`) and JwtAuth (`issuer_groups`) — write the originating group on
the **OK** access-log line only, never on a failure/`Deny *`/`Allow anon*` line.

- **OK line carries `group=<g>`** (JwtAuth also adds `iss=<iss>`): the caller already
  authenticated for that group, so the attribution is operator audit info, not a leak.
- **Failure / `Deny *` / `Allow anon*` lines MUST NOT carry `group=`** — nor `iss=` /
  `kid=` / `alg=` (JwtAuth), nor the username (BasicAuth). An unauthenticated attacker
  brute-forcing usernames / keys / issuers must not be able to watch which group or
  issuer surfaces and thereby map the multi-tenant topology.
- **Corollary (BasicAuth)**: never split `Deny bad credential` into `Deny unknown user`
  vs `Deny wrong password` — that re-introduces a username-existence oracle.

Reviewers must reject patches that add `group=` / `iss=` / `kid=` / `alg=` (or the
username) to any non-OK line. Per-plugin OK-line formats, Deny-branch lists, and code
paths live in the family files: [basic-auth](../../basic-auth/mandatory-design/group-in-ok-log.md),
[key-auth](../../key-auth/mandatory-design/group-in-ok-log.md), [hmac-auth](../../hmac-auth/mandatory-design/group-in-ok-log.md),
[jwt-auth](../../jwt-auth/mandatory-design/group-in-ok-log.md). The **inverse** convention — IpRestriction
*does* log group on its deny line, because the threat model differs — is
[ip-restriction/group-name-in-deny-log.md](../../ip-restriction/mandatory-design/group-name-in-deny-log.md).

---

## Upstream address safety

When reviewing paths that rewrite or select upstreams, load [04-review/ssrf/mandatory-design/upstream-address-safety.md](../../ssrf/mandatory-design/upstream-address-safety.md). Key distinctions:

- `domainMap` / `backendRefs` / RegionRoute PM are authorization boundaries, not equivalent to final dial-address safety.
- Production upstream final dial addresses reject loopback and link-local / metadata by default; in the Endpoint-derived path, only integration testing mode may allow loopback for local fixtures.
- RFC1918 private networks are legitimate K8s / cross-cluster / internal-API targets and must not be blanket-blocked.

---

## Historical cases

| ID | Module | Description | Conclusion |
|----|--------|-------------|------------|
| P0-04 | `csrf/token.rs` | Non-constant-time `sign` comparison | Historical closure rationale superseded — supplied nonce is visible; use current CSRF evidence above |
| P0-04b | `csrf/token.rs` | SHA-256 key mixing instead of HMAC | Closed — exact-match validation blocks length-extension exploitation |
| P0-09 | `csrf/token.rs` | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` on hot path | Closed — requires clock pre-dating 1970; `unwrap_or(0)` would silently bypass expiry check |

Review conclusions live in semantic pages under `skills/04-review/<function>/<knowledge-type>/`; search by behavior or finding ID with `rg` and update the existing rule. Follow `skills/04-review/knowledge-routing.md`.

## Historical resolution evidence: P0-04

The historical closure relied on an unknown-nonce premise. That rationale is
superseded: the nonce is supplied in the token. Use the current CSRF evidence
above and revalidate any timing finding against the actual comparison path; this
record neither proves exploitability nor suppresses a new finding.

## Historical resolution evidence: P0-04b

`csrf/token.rs` SHA-256 key mixing instead of HMAC: exact-match validation blocks length-extension exploitation; closed

## Historical resolution evidence: P0-09

`csrf/token.rs` `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()`: requires clock to pre-date 1970 (unreachable); `unwrap_or(0)` would silently bypass expiry check — panic is safer; closed

## Resolution references

- `P0-04` — —.
- `P0-04b` — —.
- `P0-09` — —.
