# Evaluate preparation of explicit DNS resolvers

Status: deferred for user decision. Source baseline: Edgion `eb70eeafd`;
Common's workspace dependency is hickory-resolver 0.26.1.

## Evidence

Gateway address selection reaches Common's dns::resolve with StandardDns, or
HTTPDNS's explicit DNS fallback. In dns/standard.rs, each nonempty-server query
parses the server strings, constructs ResolverConfig and builds a TokioResolver
before lookup_ip. dns/hickory.rs creates each server's UDP/TCP connection configs
with its configured port. parse_servers preserves accepted entries in authored
order, including duplicates; its comment saying distinct does not describe the
current implementation. This does not promise network query order.

Hickory resolver.rs::build creates a name-server pool, retry handle and response
cache. It also loads a hosts snapshot when the effective use_hosts_file option
selects Always/Auto. Construction does not imply that every configured socket is
opened immediately. The current resolver is local to one resolve_standard call;
reusing it would extend these owners beyond that call.

## Decision

Evaluate two separately scoped options, or retain the current implementation:

- Prepare parsed server addresses and immutable configuration at an existing
  configuration-generation boundary, retaining per-query resolver lifetime.
  Measure clone/conversion costs still required to build each resolver; do not
  claim that pre-parsing removes pool/cache construction.
- Prepare and reuse the full resolver under a defined generation owner. This
  additionally changes response-cache, hosts-snapshot and name-server state
  lifetimes. It is not a mechanical constructor hoist with established parity.

The Gateway's address-result cache remains separate. Reusing Hickory's internal
cache could affect fresh queries after its expiry, explicit per-request mode,
negative answers and HTTPDNS fallback. Do not silently add another global cache,
change TTL policy, introduce miss coalescing, or reuse prepared HTTPDNS's ownership
rules without checking the different StandardDns publication paths.

## Required acceptance

- Inventory every Common consumer and publication owner before freezing an exact
  cross-crate implementation allow-list. Keep ResolverSpec canonical; no separate
  Gateway-only resolver implementation or new hot-path configuration parsing.
- Preserve accepted server order/duplicates, invalid-entry skipping, bracketed
  IPv6 and per-server ports for UDP/TCP. A direct empty server list currently
  uses System, whereas an empty explicit HTTPDNS DNS fallback fails with
  NoAddresses. All-invalid nonempty input retains its current error boundary.
  Moving rejection to publication requires an explicit failure-policy decision.
- Preserve resolver selection, authored address fingerprint, request-pinned
  hostname/configuration, fallback:none, empty results and original lookup error
  classification. Old in-flight generations must not observe new server config.
- For resolver reuse, decide positive/negative caching, hosts-file refresh,
  connection/server-state retention, concurrency and reclamation on replacement
  or shutdown. Preserve per-request and Gateway cache semantics, or obtain an
  explicit behavior decision before changing them. Bound aggregate generations
  and retained resources; a per-resolver bound is not a process-wide bound.
- Use local UDP/TCP DNS integration fixtures to compare wire query counts, TTL
  expiry, answer/failure changes, TCP fallback, server replacement, concurrent
  requests and old-generation completion. Test hosts-file behavior only where
  enabled. Existing parser/cache tests do not prove this lifecycle contract.
- Measure parse/config/build costs separately from query latency, allocation,
  retained state and live-query reuse. No quantitative performance gain has been
  measured for this finding. Add no query-time tracing, metrics or payload logs.

Use IMPL after user selection. Relevant original knowledge:
[prepared HTTPDNS](../../skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md),
[address-cache lifecycle](../../skills/04-review/backends/regression-guards/externalname-dns-cache-identity-and-lifecycle.md)
and [miss-coalescing decision](../../skills/04-review/backends/intentional-design/externalname-dns-miss-coalescing-not-implemented.md).
The existing [DNS address-result ownership task](dns-01-address-result-ownership.md)
concerns returned Vec storage, not resolver preparation.

This task changes no runtime code. Fresh baseline passed 68 Common DNS and 26
Gateway DNS-cache tests, formatting, workspace check/Clippy and doc validation.
No new resolver-reuse implementation or controlled DNS wire experiment was run.
