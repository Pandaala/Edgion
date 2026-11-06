# common-02: Shared DNS layer is cacheless, rebuilds a hickory resolver per lookup, ignores record TTLs; AI upstreams resolve on every request

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: large-direction
- **Location**: `edgion-common/src/dns/mod.rs:312` (`resolve`), `edgion-common/src/dns/standard.rs:25-51` (`resolve_standard`), `edgion-common/src/dns/system.rs:17`, `edgion-common/src/dns/httpdns.rs:338` (`resolve_http_dns_prepared`), `edgion-common/src/egress.rs:22,186-245` (`EgressResolver` cache); consumer `edgion-gateway/src/ai/http/attempt.rs:188,243-262` (`resolve_ai_endpoint`), `edgion-gateway/src/backends/mod.rs:360` (`compute_routed_external_resolver`)

## Summary
`edgion_common::dns::resolve` — the canonical resolver mandated by unification rule D.4 — keeps no
state: `StandardDns` constructs a brand-new `hickory TokioResolver` (new sockets, empty cache) on every
call, `System` does a `spawn_blocking` getaddrinfo every call, and `HttpDns` issues one or two HTTPS
requests to the provider every call. Record TTLs are never read anywhere. Callers that add their own
cache use fixed TTLs and tiny/ad-hoc stores, and at least one hot path (AI provider upstream selection)
calls `resolve` directly per request with no cache at all.

## Current behavior (evidence)
- `standard.rs:51`: `TokioResolver::builder_with_config(config, TokioRuntimeProvider::default()).build()`
  inside `resolve_standard`, i.e. per lookup. Hickory's built-in LRU cache, negative caching,
  connection reuse and TTL handling are thrown away each call. `ResolverSpec::StandardDns` only holds
  `Vec<String>` (`dns/mod.rs`), so there is nowhere to keep a resolver.
- `system.rs:17`: `tokio::net::lookup_host` per call → tokio blocking pool + libc. In Kubernetes with the
  default `ndots:5`, a name like `api.openai.com` first walks every search domain (A+AAAA each) before
  the absolute query.
- `httpdns.rs:338-380`: typed mode with `DnsLookupFamily::All` performs two provider HTTP GETs per
  lookup; provider-supplied TTLs (e.g. Alibaba/Tencent HTTPDNS return `ttl`) are not parsed.
- AI path: `try_ai_backend_peer` → `resolve_ai_endpoint` (`attempt.rs:188,243`) calls
  `compute_routed_external_resolver(ctx)` and then `resolve(&spec, domain, Family(..))` for **every
  attempt of every AI request**, then takes only `.next()` (first address, no failover across the
  returned set). Unlike the ExternalName bridge, which fronts `resolve` with `DnsCache`
  (`edgion-gateway/src/backends/discovery/dns_cache.rs`), this path has no cache.
- `EgressResolver` (`egress.rs:22,225-245`): per-instance `HashMap` capped at **16** entries, fixed TTL
  (`EGRESS_DNS_TTL_MS = 5000` default), and on overflow `guard.clear()` drops *all* entries. A shared
  client that talks to more than 16 hosts (forward-auth/webhook/OIDC across tenants, Wasm callouts)
  degenerates into a resolve-per-request pattern. There is no miss coalescing, so a TTL expiry under
  load fans out N identical queries.

## Industry comparison
- Envoy: one long-lived c-ares/getaddrinfo resolver per cluster/DFP config; `STRICT_DNS`/`LOGICAL_DNS`
  refresh in the background with `respect_dns_ttl`, `dns_min_refresh_rate`, jittered refresh and
  failure back-off; the Dynamic Forward Proxy DNS cache coalesces in-flight lookups and is bounded by
  `max_hosts`. The request path never blocks on DNS once a host is warm.
- NGINX `resolver ... valid=` and Kong/OpenResty `lua-resty-dns-client` keep a process-wide,
  TTL-honoring cache with stale-while-revalidate.
- Traefik / APISIX similarly cache resolver results and honor TTL.

## Impact
- AI Gateway: an extra DNS round trip (often several, with `ndots:5`) on the critical path of every LLM
  request and retry; CoreDNS QPS scales with AI QPS; a slow resolver directly adds TTFB. With
  `StandardDns`, every request also opens fresh UDP sockets. With HTTPDNS, per-request billable provider
  calls and a hard dependency on the provider's availability.
- Records with short TTL (fail-over DNS) are not honored either way: fixed 5 s caches may serve beyond
  the record TTL, and cacheless paths ignore TTL-based stability.
- The canonical-resolver rule (D.4) currently centralizes the *selection* of a resolver but not its
  *lifecycle*, so every consumer reinvents (or forgets) caching.

## Suggested direction
Options:
1. **Minimal**: make `ResolverSpec::StandardDns` carry an `Arc<TokioResolver>` built once when the spec
   is prepared (like `PreparedHttpDns`), and route `resolve_ai_endpoint` through the existing
   `DnsCache`. Fixes the worst costs; keeps fixed-TTL semantics.
2. **Recommended**: introduce one shared, bounded `DnsCache` service in `edgion-common::dns` keyed by
   `(resolver fingerprint, name, family)`, owned once per process: TTL taken from the answer (clamped by
   operator min/max, as Envoy's `dns_min_refresh_rate`/max TTL), per-key in-flight coalescing,
   stale-on-error for a bounded window, negative caching with back-off. Make `resolve()` go through it
   by default and let `EgressResolver` and the ExternalName bridge become thin views on it (removing
   the 16-entry clear-all cache). Parse HTTPDNS TTL fields where the preset exposes them.
3. Longer term for AI/ExternalName upstreams: background refresh per configured hostname (Envoy
   `STRICT_DNS` model) so the request path only reads an `ArcSwap` of addresses and can fail over
   across all returned addresses.

## Notes
Checked `skills/04-review/backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md`
— that tradeoff explicitly relies on `DnsCache` bounding duplication on the ExternalName path; it does
not cover the AI path (no cache at all) nor the per-call hickory construction, which are new evidence.
Also checked `regression-guards/externalname-dns-cache-identity-and-lifecycle.md` and
`ssrf/regression-guards/httpdns-endpoint-egress.md`. No overlapping task under `tasks/`. Any shared cache
must keep the egress invariant that the *classified* addresses are the ones dialed (cache the filtered
set per policy, as `EgressResolver` does today).

## Related findings

- gw-ai-02 (AI endpoint selection), ctrl-svc-05 (controller DNS resolver polling).

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P1
- `edgion-common/src/dns/standard.rs:25-53`: `TokioResolver::builder_with_config(..).build()` inside `resolve_standard` on every call; empty server list falls to per-call `resolve_system` (getaddrinfo).
- `edgion-gateway/src/ai/http/attempt.rs:192-196,252-271`: `try_ai_backend_peer` → `resolve_ai_endpoint` → `resolve(&spec, domain, ..)` every attempt, then `.next()` (first allowed address only); no `DnsCache` in front.
- `edgion-common/src/egress.rs:22,225-240`: `EGRESS_CACHE_MAX_ENTRIES = 16` and `guard.clear()` on overflow.
- `skills/04-review/backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md` relies on `DnsCache` for the ExternalName path only; it does not cover the uncached AI path. P1 kept: DNS (with k8s ndots:5) is on the critical path of every AI request and retry.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1
- `edgion-common/src/dns/` and `egress.rs` are unchanged; `standard.rs` still builds a `TokioResolver` per lookup and `EGRESS_CACHE_MAX_ENTRIES = 16` still clears on overflow.
- `attempt.rs` only merged the `Skip`/`DependencyDenied` arms; `resolve_ai_endpoint` (`attempt.rs:243-262`) still calls `resolve` per attempt with no `DnsCache` and takes `.next()`. Line refs updated.
