---
name: per-request-connection-accepted
description: Use when reviewing ldap_auth "no connection pool / per-request TCP connection / fd or ephemeral-port exhaustion under cache-miss bursts" findings; explains why per-bind connections are an accepted design tradeoff and why connection-pool / semaphore proposals are not adopted (FB-04, 2026-05-27).
---

# LDAP Auth Per-Request TCP Connection — ACCEPTED-RISK

## Conclusion

`ldap_auth::ldap_bind` (`edgion-gateway/src/plugins/http/ldap_auth/plugin.rs`) opens a
fresh `LdapConnAsync::with_settings(...)` connection for every cache-miss
authentication request and `unbind`s it immediately after the `simple_bind` result
is read. There is no connection pool, no shared connection, and no in-flight
concurrency cap. This is **known, evaluated, and accepted as a design tradeoff**.

Fix suggestions of the form "introduce a `deadpool`-style LDAP connection pool",
"add a `tokio::sync::Semaphore` to cap in-flight LDAP binds", or "rebind a shared
connection across users" are **not accepted** without a re-evaluation trigger (see
below). The operator-facing mitigation is `cacheTtl` tuning, documented in the
plugin's Security Recommendations.

## Core Rationale

**1. The realistic trigger surface is narrow**

The `auth_cache` (`ShardedLruCache`) absorbs the steady-state flow: every accepted
`(username, password)` pair caches for `cacheTtl` seconds, so once the cache warms,
only first-authentication events for that pair hit `ldap_bind`. Deployments that
hit the unpooled path at scale must satisfy *all* of:

- A large active-user population whose `(username, password)` working set exceeds
  the LRU capacity (`lru_cache.capacity`, default sized via
  `get_default_lru_cache_capacity()`), or
- `cacheTtl == 0` (cache disabled — `check_cache` short-circuits to `None`, see
  `plugin.rs:125`), or
- A simultaneous cold start / cache-flush burst.

On a typical Linux gateway (~28 000 ephemeral ports, ~60 s `TIME_WAIT`), sustained
fd / port exhaustion would require >460 cache-miss binds per second indefinitely;
most LDAP backends (OpenLDAP `tool-threads`, AD LDAP connection limits) cannot
service that QPS upstream and would themselves throttle before the gateway's
ephemeral pool runs out. The "503 to all LDAP clients while ports recover" outcome
described in the original finding is reachable only at QPS well above what the
upstream LDAP server is provisioned for.

**2. LDAP simple-bind semantics make a connection pool an inherently risky refactor**

`simple_bind` is **session-rebinding**, not a per-request RPC: a successful bind
mutates the connection's authenticated identity for the lifetime of that TCP
session. A naive pool that hands out a connection, runs `simple_bind`, and returns
it must:

- Serialise binds per connection (no parallel bind on a single TCP socket — the
  identity races) — already implies a Mutex/Semaphore per pooled connection.
- Decide what state to restore on return (re-bind anonymously? unbind? keep last
  identity?) — each option has a distinct security exposure (a borrower could
  observe the previous bind's privileges if the implementation forgets the
  reset).
- Handle half-open connections from the LDAP server's idle-timeout side and
  TLS-renegotiation edge cases that the current "new connection per bind"
  pattern is naturally immune to.

The `ldap3` 0.12 crate does not expose connection-pool primitives, so this would
be net-new lifecycle code on the authentication hot path. The blast radius of a
state-leak bug here (one user's session reused under another user's identity) is
strictly worse than the current ephemeral-port exposure.

**3. The semaphore-only variant does not solve the named problem**

A `tokio::sync::Semaphore` capping `ldap_bind` to N concurrent binds would
mitigate fd / port exhaustion but:

- Does not reduce per-request latency — every cache miss still pays a full TCP
  (and optional TLS) handshake plus the `2 × timeout` upper bound.
- Adds a queue that can itself become the bottleneck under the same burst
  scenario the finding warns about: requests beyond the cap wait up to
  `2 × timeout` for a permit, surfacing as the same 503 spike, just from a
  different source.
- Requires a new config field (`LdapAuthConfig.max_concurrent_binds` or
  similar) crossing the types crate / controller schema / gateway runtime —
  scope-wise no longer a narrow plugin-local change. The current scope-gate
  rules in `fix-review-issue.skill` flag this as wide-impact.

The cost/risk is therefore disproportionate to the residual hazard once
`cacheTtl > 0` is set — which the existing Security Recommendations already
require.

**4. The operator-facing mitigation already exists**

The plugin's Security Recommendations (`docs/{en,zh-CN}/user-guide/edgion-plugins/ldap-auth.md`)
already prescribe a non-zero `cacheTtl` (60–300 s suggested) and stacking
`RateLimit` to bound probe rates. Both are sufficient for any deployment whose
LDAP backend is itself sized for the request rate. `cacheTtl: 0` is documented as
the cache-disable knob with the explicit understanding that *every* request will
incur an LDAP round-trip — operators choosing it accept the connection-cost
implication.

## Fix Suggestions Not Accepted

- Introduce a `deadpool` (or any other crate-level) LDAP connection pool — see
  rationale §2. Rebinding semantics + identity-leak risk + new dependency are
  out of proportion to the residual exposure, and `ldap3` 0.12 does not provide
  the pool primitives required to do it safely.
- Add a `tokio::sync::Semaphore` to cap in-flight `ldap_bind` calls — see
  rationale §3. The semaphore moves the exhaustion symptom from fd/port to
  queue depth without reducing per-cache-miss latency, and requires a
  config-schema change that breaks the narrow-fix scope of the review workflow.
- Make `ldap_bind` reuse a single shared connection and rebind it across users —
  rationale §2 — this is the worst of the proposals: it serialises *all* LDAP
  auth through one socket and is the most likely path to a cross-user identity
  leak.
- Re-rate the finding as High because "503 to all clients" is theoretically
  reachable — not accepted; the precondition (sustained binds at a rate
  exceeding upstream-LDAP-server provisioning) is upstream-bound first.

## Re-evaluation Triggers

Re-open this decision only if:

- A production deployment reports fd / ephemeral-port exhaustion attributable to
  `ldap_auth` with a non-trivial `cacheTtl` configured (i.e. the cache is not
  effectively disabled).
- The `ldap3` crate or a successor exposes a safe pooled-connection primitive
  with first-class per-bind identity-reset semantics — at that point the
  rationale §2 objection collapses and a pool becomes a sound refactor.
- `ldap_auth` grows a non-credential operation (e.g. a directory-search step)
  that *does not* mutate connection identity — those operations would pool
  trivially and the absence of pooling becomes a clearer regression.

## Reference Cases

- F-B04 (Medium, Performance), accepted ⚠️ tradeoff on 2026-05-27 —
  `tasks/plugins/plugins-http-FB04-ldap-auth-per-request-new-tcp-connection.md`
  (closed).
- Source: `edgion-gateway/src/plugins/http/ldap_auth/plugin.rs` (`ldap_bind`,
  `check_cache`, `store_cache`).
- Sibling decisions:
  [discriminated-error-log-tags.md](discriminated-error-log-tags.md) (other
  `ldap_auth` accepted finding) and
  [../auth-timing/ldap-auth-cache-timing-oracle.md](../auth-timing/ldap-auth-cache-timing-oracle.md)
  (the cache-hit timing tradeoff that the operator mitigation in §4 is shared
  with).
- Doc: `docs/{en,zh-CN}/user-guide/edgion-plugins/ldap-auth.md` Security
  Recommendations (`cacheTtl` tuning + `RateLimit` stacking — the existing
  guidance is the canonical operator mitigation, no per-request-connection
  note was added there since the `cacheTtl` recommendation already covers the
  operational path).
