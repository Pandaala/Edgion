---
name: externalname-dns-miss-coalescing-not-implemented
description: Use when reviewing concurrent ExternalName DNS resolution; covers why the cached self-resolve path deliberately does not coalesce misses or negative-cache failures, and which trigger conditions would reopen that decision.
status: accepted-tradeoff
finding_id: task-externalname-dns-miss-coalescing
closed: 2026-09-03
knowledge_type: intentional-design
---

# ExternalName DNS Miss Coalescing Is Not Implemented (task-externalname-dns-miss-coalescing) — DESIGN-TRADEOFF

## Conclusion

`resolve_external_ips` (`edgion-gateway/src/backends/mod.rs`) checks `DnsCache`, and on a miss calls
`edgion_common::dns::resolve` directly. Concurrent callers for the same key therefore each issue
their own resolver call, and a failed resolution is neither cached nor backed off, so the next
request retries immediately. **This is accepted and will not be fixed on the current trigger
surface.**

Fix suggestions of the form "add a per-key pending-query registry with a global admission budget
and a fail-closed overflow result", "add a negative/failure backoff state machine with cancellation
ownership rules", or "port Envoy's dynamic-forward-proxy DNS cache load coordination" are **not
accepted** today. The re-evaluation triggers below define when they become worth revisiting, and
the shape section defines what a future fix should look like — which is materially smaller than
any of those.

## Core Rationale

**1. The default deployment never executes this path at all**

`mode=controller` is the default (`DnsResolutionMode::default()`), and in K8s conf_center mode the
controller's leader-only background loop
(`edgion-controller/src/services/dns_resolver/service.rs`, `enabled: true`, `refresh_interval: 15s`)
resolves those Services centrally and publishes real EndpointSlices. `decide_external_name_path`
then returns `ExternalNamePath::EndpointSlice` and the self-resolve branch is not entered.

The cached self-resolve path is reached only under a non-default choice:

| Trigger | Nature |
|---|---|
| `edgion.io/dns-resolution-mode: gateway` | Explicit operator opt-in; a first-class mode that *never* consults the slice |
| FileSystem / Etcd conf_center | The controller DNS resolver service is never started there, so `mode=controller` permanently uses the bridge |
| `mode=controller` with no ready slice endpoint | Genuine degradation: controller down, not yet leader, before the first tick, or control-plane resolution failed |

Only the third row is a "controller is broken" window, and in that window the deployment is already
degraded on a broader axis than DNS query amplification.

**2. Where the path *is* reached, `DnsCache` already bounds the duplication**

The 5s-default cache (`DEFAULT_DNS_CACHE_TTL`, EGC `dnsResolver.cacheTtl`) means the steady-state
cost is a small burst of duplicate queries at each TTL boundary, sized by `resolution_latency × QPS`
— single digits for a typical resolver. That is not a cost worth a new hot-path concurrency
primitive.

The genuinely bad shape needs **two** conditions at once: a non-default resolution mode *and* an
unhealthy upstream resolver. Only then does the missing negative backoff turn every request into
its own full DNS timeout. Note that `mode=per-request` is explicitly excluded from this finding —
it opts out of caching by definition, so coalescing would contradict its contract.

**3. The nginx / OpenResty precedent argues the opposite way once the control plane is accounted for**

Both ecosystems did build exactly this mechanism, which is worth stating precisely so the precedent
is not mis-cited in a future review:

- nginx's `ngx_resolver.c` implements its own DNS client (deliberately not `getaddrinfo`, which
  blocks), with a per-name cache whose TTL is overridable via `resolver ... valid=`, **and** real
  miss coalescing: `ngx_resolve_name` attaches a new context to an existing in-flight node's waiting list
  instead of sending a second query.
- OpenResty's `lua-resty-dns` has neither cache nor coalescing, which is why the ecosystem layers
  `lua-resty-lrucache` + `lua-resty-lock` + `lua-resty-mlcache` on top; `mlcache` uses the lock
  precisely to guarantee a single callback execution per miss. Kong's `kong.resty.dns.client`
  keeps its own per-name in-flight table for the same reason.
- Envoy's dynamic-forward-proxy DNS cache likewise coordinates pending queries and bounds
  admission.

In all three, DNS resolution is the **only** path to an upstream address; there is no control plane
resolving names centrally and publishing endpoints. Edgion has one, and it is the default. The
precedent establishes that the mechanism is sound engineering, not that Edgion's default path needs
it.

**4. The cost lands on the request hot path**

The fix adds shared state and an additional await point inside the `upstream_peer` path. Weighed
against a `minor`-to-`moderate` harm that only materializes under a non-default configuration, the
risk/benefit does not clear the bar. Recorded as `Value verdict: 🔴 Not worth fixing as specified`.

**5. Secondary costs that a future fix should account for (not reasons to fix now)**

If this is ever reopened, these belong in the analysis and were surfaced during this review:

- `ResolverSpec::System` runs `getaddrinfo` through `tokio::net::lookup_host`, i.e. on the tokio
  **blocking pool** (default cap 512 threads). Under a resolver outage, N duplicate concurrent
  lookups occupy N blocking-pool slots for the full timeout, which degrades unrelated blocking work.
  No upstream DNS cache mitigates this.
- A pod's `ndots:5` search list expands one logical resolution into several wire queries, so the
  duplication factor multiplies.
- glibc's `getaddrinfo` does **not** cache on its own. In a typical pod neither nscd nor
  systemd-resolved is present, so "the host caches it anyway" is not a valid assumption. CoreDNS's
  `cache` plugin and NodeLocal DNSCache do exist, but they are a network hop away and save neither
  the round trip nor the blocking-pool slot.

## Shape To Use If This Is Reopened

Do **not** implement the original task's design (per-key in-flight registry + global pending budget
+ fail-closed overflow + cancellation-ownership rules). That is Envoy's shape, sized for a system
where the DNS cache is the sole resolution path.

The proportionate shape for Edgion is the one already proven in-tree by the OIDC JWKS provider
(`edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs`, covered by its
`20 concurrent get_jwks ⇒ exactly 1 upstream fetch` test):

- A `tokio::sync::Mutex` scoped to the cache key, plus a **re-check of the cache after acquiring
  it**. N cold callers produce one resolver attempt; the rest hit the cache on re-check. (In the
  JWKS case the mutex is a `Mutex<()>` field on the provider instance, and a provider is constructed
  once per issuer config, so it is per-key by construction rather than by an explicit key map. A DNS
  version needs the key-scoping to be explicit, since one process serves many keys.)
- Cancellation safety is structural: if the leader is cancelled, the guard drops and the next waiter
  re-checks and takes over. No waiter is stranded and no waiter's work is cancelled by another
  task's cancellation — no explicit ownership protocol is needed.
- Failure handling is a short negative/backoff entry, not a state machine: the leader records it,
  and waiters that see it return the typed failure immediately instead of each paying a full
  timeout.

This needs no admission budget and no overflow result. Be precise about why: the `ShardedLruCache`
capacity bounds *resident* entries, **not** the number of distinct keys that can be in flight at
once — a cold burst across more distinct hostnames than the capacity would still produce that many
concurrent resolutions. The reason a budget is unnecessary is different: that concurrency ceiling
already exists today, set by the gateway's in-flight request concurrency, and coalescing can only
ever *lower* it. An admission budget would be bounding something the fix does not make worse, at
the price of a new fail-closed error path on the request hot path.

## Fix Suggestions Not Accepted

- **Per-key pending registry + global admission budget + fail-closed overflow** — Envoy-sized for a
  trigger surface the default deployment never reaches. The budget also bounds a quantity coalescing
  does not make worse: concurrent resolutions are already capped by in-flight request concurrency
  today, and adding a leader/waiter shape can only lower that count (see the Shape section — the
  bound is *not* the cache capacity, which governs resident entries rather than in-flight keys).
- **Shared-future (`Shared<Future>`) in-flight registry** — adds registry lifecycle, cancellation
  propagation, and error-cloning constraints to a request hot path, for a `minor` steady-state win.
- **Negative-cache DNS failures inside `DnsCache` itself** — would put a failure marker under the
  same key as a success and risk it surviving a generation change; the identity contract in
  [externalname-dns-cache-identity-and-lifecycle.md](../regression-guards/externalname-dns-cache-identity-and-lifecycle.md)
  is deliberately about resolved addresses only.
- **Extend coalescing to `mode=per-request`** — that mode's contract is "re-resolve every request";
  coalescing would silently violate it.
- **Rely on the host / CoreDNS cache instead** — see rationale 5; not a valid assumption in a pod,
  and it does not address the blocking-pool or timeout-amplification costs.

## Re-evaluation Triggers

Re-open this decision only if:

- `edgion.io/dns-resolution-mode: gateway` becomes a production deployment shape with meaningful
  QPS, rather than an opt-in escape hatch.
- FileSystem / Etcd conf_center becomes a primary deployment target, making the self-resolve bridge
  the normal path for `mode=controller` Services.
- A DNS-outage incident shows request-latency or blocking-pool amplification traceable to the
  missing negative backoff — the backoff half is independently useful and could land alone.
- The controller-side DNS resolver service is removed, disabled by default, or otherwise stops being
  the default resolution path; rationale 1 collapses entirely if that happens.

## Reference Cases

- Origin: a `tasks/todo/externalname-dns-miss-coalescing.md` entry, never committed to git and
  deleted on closure (2026-09-03) without implementation. Nothing else points to it: its
  requirements are the ones restated and rejected above, so this file is the complete record.
- Prerequisite (implemented): [externalname-dns-cache-identity-and-lifecycle.md](../regression-guards/externalname-dns-cache-identity-and-lifecycle.md).
- Source: `edgion-gateway/src/backends/mod.rs` (`resolve_external_ips`),
  `edgion-gateway/src/backends/discovery/dns_cache.rs`,
  `edgion-controller/src/services/dns_resolver/service.rs`.
- Reference shape: `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs`.
- Related follow-up: `tasks/todo/reuse-dns-resolver-instances.md` (per-call resolver/fetcher
  construction, a separate and equally configuration-gated cost).
- External precedent: Envoy dynamic forward proxy DNS cache
  (<https://github.com/envoyproxy/envoy/blob/main/source/extensions/common/dynamic_forward_proxy/dns_cache_impl.cc>),
  nginx `ngx_resolver.c`, OpenResty `lua-resty-mlcache` + `lua-resty-lock`.

## Resolution references

- `task-externalname-dns-miss-coalescing` — 2026-09-03 (accepted tradeoff; not implemented).
