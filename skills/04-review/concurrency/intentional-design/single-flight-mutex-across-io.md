---
name: single-flight-mutex-across-io
description: Use when reviewing an async mutex held across a network round trip; covers the narrow exemption from design rule V-1 for data-free per-key single-flight mutexes (DNS answer cache, OIDC JWKS and token refresh) and how to tell an exempt shape from a violation.
status: accepted-design
finding_id: common-02 (P6 review, rule V-1)
closed: 2026-11-06
knowledge_type: intentional-design
---

# Data-free single-flight mutex held across I/O — exempt from V-1

## Conclusion and scope

Design rule V-1 (`skills/00-design-rules.md` §V) forbids holding an async lock guard across a
network round trip, because the guard blocks every other reader of the protected state for the
RPC's duration. A **per-key single-flight mutex** is exempt when all of these hold:

1. The mutex guards **no data** (`tokio::sync::Mutex<()>`); it exists only to make same-key work
   run once.
2. It is **scoped to one key** (one hostname/resolver/policy, one issuer, one refresh token), so
   unrelated keys never wait on it.
3. Every waiter **re-checks the cache after acquiring it** and normally returns without doing the
   I/O itself; the waiting is the intended coalescing, not collateral blocking.
4. Cancellation is structural: a cancelled holder drops the guard and the next waiter proceeds.
5. The I/O inside is bounded by its own deadline, and the registry of keys is bounded (or the
   mutex lives on a per-key owner). For the DNS cache: hickory's per-query timeouts (Standard
   DNS), the HTTPDNS connection's request timeout, and a 30 s bound on waiting for the System
   resolver (`edgion-common/src/dns/system.rs`, added 2026-11-07; `getaddrinfo` has no caller
   deadline and keeps running on its blocking thread after the wait is abandoned).

Current users: `edgion-common/src/dns/cache.rs` (`DnsAnswerCache` cold misses, via
`edgion_common::singleflight::FlightRegistry`), the OIDC JWKS provider
(`edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs`) and OIDC token refresh
(`FlightRegistry` in `openid_connect/plugin.rs`). Envoy's dynamic forward proxy uses the same
pattern: requests for an uncached host pause until the single resolution completes.

Anything else — a lock that protects state other readers need, a client handle behind a lock, a
lock shared across keys — remains a V-1 violation and should be fixed with the canonical
clone-and-drop shape in
[async-write-lock-across-rpc.md](../regression-guards/async-write-lock-across-rpc.md).

## Rationale and evidence

The cost V-1 guards against is unrelated readers stalling behind I/O. With a data-free per-key
mutex, the only parties that wait are callers that need exactly the result being produced, and
they would otherwise each issue the same I/O (thundering herd on a cold or expired key). Removing
the wait means removing coalescing. The alternative shape — a spawned leader task plus a
notification channel — was rejected for the DNS cache during spec review
(`docs/superpowers/specs/2026-11-06-shared-dns-answer-cache-design.md`, D3): it adds detached
tasks, a drop-guard protocol and an admission budget without reducing anyone's wait.

## Alternatives and limits

- Accepted cost: if the holder's I/O hangs until its deadline, same-key waiters wait too, and a
  cancelled holder stores nothing, so waiters may resolve one after another. Each waiter is
  bounded by its own request deadline.
- Not covered: waiting on a lock to read or mutate shared state while doing I/O; using one
  mutex for many keys; unbounded key registries.

## Verification and re-evaluation

Verified 2026-11-06: `dns::cache::tests::concurrent_cold_misses_share_one_resolver_call` and
`queued_waiter_takes_over_from_a_cancelled_leader`; the existing OIDC JWKS concurrency test
(20 concurrent fetches → 1 upstream fetch). Re-evaluate if a single-flight user starts guarding
data with the mutex, if profiles show same-key waiting dominating latency, or if V-1 is reworded.

## Resolution references

- `common-02` P6 review (rule V-1 conflict) — 2026-11-06, exemption decided by the user.
- Historical source of the shape: [ExternalName miss coalescing (superseded)](../../backends/superseded/externalname-dns-miss-coalescing-not-implemented.md).
