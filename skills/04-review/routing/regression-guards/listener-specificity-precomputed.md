---
name: listener-specificity-precomputed
description: Use when reviewing listener hostname specificity (`has_more_specific` on `GatewayInfoIndex` / `GatewayListeners`) — why the exact/wildcard/catch-all shadowing facts are built with the immutable listener index instead of scanning listeners per request, the small-inventory linear/hash threshold, and which semantics the summary must preserve.
status: fixed
finding_id: routing-02
closed: 2026-11-07
knowledge_type: regression-guards
---

# Listener hostname specificity is precomputed with the listener index — FIXED

## Conclusion and scope

`GatewayInfoIndex::has_more_specific` (cross-Gateway scope, used by
`check_gateway_listener_match` for HTTP, gRPC, TLSRoute and stream route matching) and
`GatewayListeners::has_more_specific` (per-Gateway scope, used by the HTTPS
`requireSniHostMatch` 421 check in `pg_request_filter.rs`) answer from a
`HostnameSpecificity` summary in `edgion-gateway/src/runtime/store/listener_gateway_info.rs`.
The summary is built by `GatewayInfoIndex::add` / `GatewayListeners::add` while the
index is constructed (`from_entries`, `ListenerGatewayInfoStore::rebuild`) and lives in
the same immutable `Arc` as the listeners, so it is replaced atomically with the
snapshot and old requests keep their own. There is no second `ArcSwap`, global cache,
or request-time lazy build; see the
[merged listener snapshot guard](../../concurrency/regression-guards/listener-gateway-info-dual-arcswap-merged-snapshot.md).

The summary holds the `*.suffix` wildcards sorted longest first and the
ASCII-lowercased exact hostnames, plus a hash index over the exact hostnames only when
there are more than `EXACT_HASH_THRESHOLD` (16) of them. Queries are:

- current listener exact → `false`;
- current catch-all → `true` iff any wildcard or exact listener matches;
- current wildcard → `true` iff the first (longest) matching wildcard is strictly
  longer than the current one, or an exact listener equals the request host
  (ASCII-case-insensitive).

Wildcards are checked before exact hostnames because the common catch-all shape then
short-circuits on a single suffix comparison.

## Rationale and evidence

Listener hostnames change only on Gateway publication, but the former implementation
linearly scanned every listener on the physical key (and every Gateway) for each
candidate parent-ref check whenever the current listener was a wildcard or catch-all,
making per-request cost O(listeners × candidates) for catch-all/multi-tenant setups.
Precomputing at publication follows the project rule of resolving static policy before
the request hot path.

Measured evidence (one-off 2026-11-07 out-of-tree release-mode microbenchmark, not
reproducible from a repository command, reproducing the
former scan, an always-hash summary and the shipped hybrid over a catch-all +
`*.example.com` + N exact listener inventory; ns per call, single core, noisy ±3 ns):

| exact N | former scan | always-hash | shipped hybrid |
|---|---|---|---|
| 0–4 | 5–12 | 18–29 | 4–9 |
| 16 | 7–24 | 22–27 | 5–14 |
| 64 | 4–72 | 18–20 | 4–20 |
| 256 | 5–240 | 17–21 | 4–20 |

An always-hash exact set regressed small inventories by ~15–25 ns per call (SipHash on
the request host costs more than a few length-mismatched comparisons), which is why the
linear path is kept up to the threshold. The hybrid was not slower than the former scan
at any measured size and bounds the large-inventory case.

Preserved semantics (locked by the differential test below):

- Exact > wildcard > catch-all; only a strictly longer wildcard shadows, so
  equal-length different wildcards and an identical hostname never shadow.
- Exact matching stays ASCII-case-insensitive (non-ASCII bytes compare exactly);
  wildcard suffix matching keeps the raw, case-sensitive, multi-label
  `radix_hostname_matches` semantics. A `*` pattern not in the `*.` form never matches
  and is omitted.
- Every admitted listener contributes, including listeners with no eligible Route or
  whose AllowedRoutes deny the Route; they still shadow less-specific listeners.
- The cross-Gateway (route matching) and per-Gateway (SNI 421) scopes remain distinct
  summaries.

HTTP request hostnames are lowercased by `NormalizedHostname` (TLSRoute and stream
callers normalize SNI separately), so above the threshold the exact check is normally a
single allocation-free hash lookup; a mixed-case query falls back to the linear
case-insensitive scan to keep the public method's contract.

## Alternatives and limits

- Reusing `edgion_algo` `RadixHostnameMatcher` was not chosen: it requires validated
  lowercase patterns and queries, which would change behavior for unvalidated or
  mixed-case listener hostnames that the raw predicate currently accepts.
- A faster non-SipHash hasher was not adopted: it needs a new dependency and gives up
  hash-flooding resistance for no measured need.
- The summary duplicates listener hostnames (one copy per Gateway, one for the whole
  key, plus the hash index above the threshold). This is cold-path memory bounded by
  the listener count; wildcard insertion is O(n²) on rebuild only.
- Wildcard lookup still scans wildcards (longest first) until the first match.

## Verification and re-evaluation

- `cargo test -p edgion-gateway --lib -- listener_gateway_info`:
  `specificity_summary_matches_the_linear_scan_for_both_query_scopes` compares both
  scopes against a verbatim copy of the former scan across empty, catch-all, exact,
  nested and reverse-inserted wildcard, mixed-case, non-ASCII upper/lower, empty, bare
  `*.`, malformed-star, equal-length, duplicate, above-threshold and multi-Gateway
  inventories; `exact_hash_index_is_built_only_above_the_threshold` covers the switch.
- `actual_entry_421_more_specific_listener_scope` (request-entry suite): a same-Gateway
  exact listener yields 421, while another Gateway's exact listener yields no 421 but
  still shadows the wildcard listener's Route.
- `cargo test -p edgion-gateway --lib -- actual_entry route sni listener_gateway_info
  pg_request_filter` — 1134 passed on 2026-11-07. An independent out-of-repo
  byte-level model of the former scan and the shipped hybrid (25 hostname tokens,
  inventories of up to 3 listeners, 11.9M inventory/request/current cases per run)
  found no mismatch at hash thresholds 0, 1, 2 and 16. This is model evidence, not a
  repository test.

Re-evaluate if listener specificity gains a new tier, if listener hostnames become
validated/normalized at publication (the summary could then reuse the shared radix
matcher), if wildcard listener counts per key grow large enough to need a suffix
index, or if a hot-path profile shows the threshold should move.

## Resolution references

- `routing-02` (P3) — fixed 2026-11-07. History: the deferral record
  `task/review-data-panel/routing-02-listener-specificity-index.md` (added in
  `eb93bbcdb`, removed in `43a356baf`); the reopened task under `tasks/todo/` was never
  committed and was removed on completion.
- Historical duplicate-algorithm decision: [listener-specificity-duplicated](../superseded/listener-specificity-duplicated.md).
