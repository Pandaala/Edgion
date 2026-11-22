---
name: dns-endpoint-table-lifecycle
description: Use when reviewing the Gateway DNS endpoint table for ExternalName Services (gateway mode), its reference-driven lifecycle and GC, refresh policy, change suppression, the coordinator's Gateway-local slice partition, or the per-Service DNS mode decision.
knowledge_type: regression-guards
---

# Gateway DNS endpoint table lifecycle (DNS resolution redesign)

## Conclusion and scope

`ExternalName` Services in `gateway` mode (the default) are resolved by the
process-wide endpoint table in `edgion-gateway/src/backends/discovery/dns_table/`
(`state.rs` synchronous core, `policy.rs` refresh math, `mod.rs` global table and
worker). The full normalized address set becomes Gateway-local synthetic
EndpointSlices (`edgion.io/endpoint-origin: gateway-dns`), so ordinary LB, health
check, outlier ejection and sticky sessions apply on HTTP, gRPC, TCP, UDP and TLS.
These invariants must hold:

1. **Reference-driven, never request-driven.** Entries (key = normalized hostname +
   `ResolverSpec::fingerprint` + `DnsLookupFamily`) are created, shared and released
   only by Service events (`ServiceHandler` full/partial), EGC snapshot publication
   (`on_settings_changed`) and LinkSys dispatch (`on_resolver_sources_changed`). The
   request path only calls `wait_first_answer`, which never inserts. Table size is
   therefore bounded by configuration, plus the hard `maxEntries` cap that rejects new
   targets and never evicts referenced ones.
2. **No residue.** An entry is deleted with its last referrer; a Service delete, mode
   switch, hostname change or leaving `ExternalName` withdraws its slices in the same
   mutation. A resolver or family change for the same hostname is make-before-break:
   the binding keeps the previous entry as `draining` (still referenced, never
   refreshed: `take_due` and `next_due` share the `is_targeted` filter, or an overdue
   bridged entry would keep the worker waking for nothing) and publishes its answer,
   filtered to the new family, until the new entry's first result of any kind; then the
   previous reference is dropped. An answer with no address in the new family never
   bridges, and a bridge whose answer empties later (shared entry refreshed by another
   Service, an in-flight result, `expire_stale`) ends in `answer_changed`; either way
   the Service waits for the first answer instead. The cap check of a switch does not
   count entries the switch itself releases (referenced by that Service alone). A slot
   freed when a switch ends is offered to Services refused at the cap
   (`retry_rejected_if_freed` after every batch of results and every worker step). A
   failing first result withdraws the endpoints, so a LinkSys change that re-keys the
   target (endpoint policy, failed preparation) takes effect within one lookup and no
   new lookup goes through the replaced resolver (guarded by the `DnsResolutionFlap`
   `to_blocked` / `to_invalid_connection` phases); a lookup already in flight at the
   switch may still land on the bridged entry. A LinkSys change that keeps the
   fingerprint (for example a rotated Secret) swaps the entry's spec in place and is
   used from the next scheduled refresh. A drained
   Service is excluded from the pending-waiter map. A Service relist is a mark-and-sweep. A periodic worker
   pass (`reconcile_published`) withdraws any Gateway-local slice the table does not
   own. `store-stats.dns_endpoint_table` counts return to zero once no Service uses the
   table, which the integration residue check observes.
3. **Change suppression.** Answers are normalized by
   `edgion_common::dns::endpoint_slices::normalize_addrs`; an equal set updates only
   timestamps and makes no publisher call, and the coordinator additionally treats an
   identical republish as a no-op. `BatchingPublisher` merges publications per Service
   (last wins), so a withdraw followed by an identical republish in one mutation never
   reaches the coordinator as a change; the worker applies every completed lookup of
   one wake-up in one mutation (one coordinator transaction). `activate_service` rebuilds endpoint runtime state on
   every membership change, so suppression is what keeps health and outlier state across
   refreshes.
4. **Partition.** Gateway-local slices live in the coordinator map under the
   `gateway-dns:` key prefix. `replace_endpoint_slices` (conf_sync relist) keeps them,
   conf_sync updates cannot address them, and `admitted_service_slices` feeds an
   `ExternalName` in `controller` mode only from conf_sync slices, every other
   `ExternalName` only from Gateway-local slices, and every other Service only from
   conf_sync slices without `edgion.io/endpoint-origin: controller-dns` (a Service
   switched away from `controller` mode never routes to the Controller's resolved
   addresses while their withdrawal is in flight). Epoch preflight and projection use
   the same admitted list.
5. **No cross-source fallback.** `gateway` never falls back to per-request DNS and
   `controller` never bridges to Gateway resolution. The mode decision is the pure
   `backends::external_name_source` (annotation, else EGC `endpointTable`); an invalid
   annotation fails closed.
6. **Failure semantics.** Success replaces the set; `NoAddresses` withdraws at once;
   other failures keep the last good set while `now - failing_since < maxStaleSeconds`
   (`0` keeps nothing, not even at the first failure), where `failing_since` is the
   first failure after the last success (counting from the last success would leave
   almost no tolerance once the refresh interval approaches `maxStale`). The deadline is
   part of `next_due`, and every worker step runs `expire_stale` first, so it holds
   while a retry is queued, in flight or behind a long back-off, and a lowered
   `maxStaleSeconds` applies at once. An answer that normalizes to nothing (only
   link-local or unspecified addresses) is `Gone`, like `NoAddresses`. Back-off starts
   at `minRefreshSeconds` and doubles to `min(60 s, maxRefreshSeconds)`, never below
   `minRefreshSeconds` (so with `minRefreshSeconds` above 60 it is flat). Results for a deleted or recreated entry are
   discarded by generation. A refresh-policy change pulls every scheduled refresh in to
   at most `latest_success_delay()`; an invalid refresh block runs the defaults.

## Rationale and evidence

User decisions of 2026-11-10 (`tasks/todo/dns-resolution-redesign/`): Envoy
`STRICT_DNS` shape as the default, a table with explicit lookup and automatic expiry
that can never leak entries for vanished Services or domains, `TTL x factor` refresh
with configurable bounds (minimum 2 s, maximum the RFC 2181 TTL), and no update when the
sorted address list is unchanged. The previous request-driven `gateway` mode rotated a
cached answer and dialed one address, with no pool, health state or L4 support.

## Alternatives and limits

- Rejected: per-request fallback when local endpoints are missing (hides table bugs,
  multiplies DNS load during outages); the bounded first-answer wait (2 s) covers cold
  start instead.
- Rejected: Gateway-resource DNS annotations for `ExternalName` (one Service would map
  to a different key per Gateway, so Service events alone could not drive the table).
- Rejected: LRU eviction at the cap (would break working traffic).
- Accepted: the OS resolver reports NXDOMAIN as a transient error, so a removed name
  stays until `maxStaleSeconds`. Refresh concurrency is bounded at 16, each query at 10 s;
  pending first answers are taken before overdue refreshes.
- The worker starts after the Gateway's initial sync
  (`EdgionGatewayCli::run_after_logging`, after `wait_for_ready`): entries bound
  earlier stay pending, so nothing is resolved under pre-configuration inputs (default
  resolver or family before the EGC or a LinkSys). Listeners are created after
  `wait_settled`, which gives those first answers at most `FIRST_ANSWER_WAIT`.
- A Service relist updates the table before the coordinator (`full_set`, like
  `partial_update`), so a Service whose hostname or mode changed while disconnected
  never joins its previous Gateway-local slices.
- Not implemented: Prometheus metrics for the table (admin view and store-stats only).
- Accepted: the first-answer wait is a fixed 2 s, not capped by the route's connect
  timeout, and it happens at most once per request or TLS connection
  (`dns_first_answer_waited` on `EdgionHttpContext` / `TlsContext`, set only when a
  wait actually happens); a request deadline that passes during it answers 504
  (`expired_deadline`). There is no dedicated "DNS pending" error; after the wait, ordinary
  selection reports the no-ready-endpoint error. A wake only means the pending set
  changed, so `wait_first_answer` re-checks its own Service and keeps waiting on its
  current signal until the one deadline.
- Accepted: the UDP datagram path never waits on a pending entry (the datagram is
  dropped like one sent to an empty Service). `system`-mode UDP sessions use the
  hostname-keyed single-flight shared answer cache (`backends::datagram_lookup`): a
  cached answer is served at the first poll, an uncached one gets a 250 ms budget, and
  while a lookup of that name is known to be slow further new sessions are dropped at
  once, so spoofable new sources cannot multiply DNS queries and a stalled resolver
  costs the receive loop one budget per lookup, not one per datagram.
- Normalization drops link-local (169.254.0.0/16, fe80::/10) and unspecified (0.0.0.0,
  ::) answers so metadata addresses and "this host" never become LB or health-probe
  targets; loopback is kept and gated per Gateway by `allowLoopbackUpstream` at
  selection time, and active health checks of an `ExternalName` Service never dial it
  (`health::check::manager::refuses_dns_endpoint`, every probe type; outside
  integration testing): DNS answers bypass the apiserver's EndpointSlice validation
  (CVE-2021-25737), which ordinary Services rely on. Envoy itself has no such check;
  Envoy Gateway rejects loopback Backend addresses at configuration time. `system` mode drops unspecified answers before selection on every
  protocol (`backends::drop_unspecified`), because the backend address check does not
  classify them.
- Accepted: settings are process-wide (the smallest-named EGC defining `dnsResolver`);
  a Service-keyed table cannot hold per-Gateway settings. Several defining EGCs log a
  warning on every EGC publication.
- Accepted: an OS lookup that outlives `RESOLVE_TIMEOUT` keeps its blocking thread
  until the resolver's own limit, although its permit is released.
- `system`-mode lookups are bounded by the caller's budget (`backends::within_budget`):
  the L4 connect timeout for TCP/TLS (the connect then gets its own timeout, so setup
  can take up to twice it), the remaining request timeout and gRPC deadline for
  HTTP/gRPC, where running out is that deadline's 504, not a DNS failure. An HTTP
  answer left empty by `drop_unspecified` records "dns answer had no usable address".

## Verification and re-evaluation

Unit tests: `tests/unit/backends/discovery/dns_table/{state_test,policy_test,mod_test}.rs`
(lifecycle, sharing, mark-and-sweep, cap and retry, stale generations, failure table,
change suppression, bounded waits), `coordinator_test.rs`
(`gateway_dns_slices_feed_only_non_controller_external_names`,
`conf_sync_relist_and_updates_cannot_touch_gateway_dns_slices`,
`identical_gateway_dns_republish_keeps_the_generation`), and
`mod_test/native_service_dispatch_tests.rs` (`external_name_source_*`,
`modes_never_fall_back_and_controller_readiness_keeps_runtime_health`). Integration:
`DnsResolution` (L7 in every mode, TCP in every mode, UDP in `system` mode) and the
`DnsResolutionFlap` dynamic scenario.

For `externalname-default-dns-resolution-mode`, keep the mode annotation absent on
the `dns-l4-gateway` Service. The TCP echo and endpoint-table membership checks
together verify that a TCP backend inherits the default and uses Gateway-local DNS.

Re-evaluate when a new event source can change a target key, when request paths gain a
way to create entries, or when the coordinator's slice partition changes.

## Resolution references

- DNS resolution redesign, implemented 2026-11-10; supersedes the request-driven
  `gateway` / `per-request` modes and the Controller-mode bridge.
