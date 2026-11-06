---
name: h2-proxy-cache-reuse-harness-consumers
description: Historical review evidence for Pingora fork / H2 proxy cache-admission and upstream reuse; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 proxy cache reuse harness consumers

## Recorded conclusion

The H2 audit's proxy test harness (cache entry inspection, injected H2 windows, local response-body failure, upstream socket observability) had no end-to-end callers, so cache admission, connection reuse and local-failure isolation were unproven. Fixed by the new standalone target `pingora-proxy/tests/test_h2_upstream_cache_and_reuse.rs` (7 scenarios) asserting on cache state, origin socket identity, and the upstream handshake windows rather than downstream keepalive. Adding a dedicated CI job, relocating the coverage into `edgion-tests` (the Edgion Gateway implements no HTTP response cache — no manifest declares `pingora-cache` and `edgion-gateway/src/` never touches `session.cache` — so cache admission never runs and the contracts have no subject there), asserting reuse from keepalive alone, asserting the connection window through a behavioural flow-control scenario (it is pinned at the handshake instead, and any value at or below 65535 is a no-op), and claiming the reuse assertion proves no pump task leaked are not accepted.

Current-state correction (source inspection, 2026-11-06): the parenthetical
above predates Edgion's own response cache. Edgion now has an Edgion-owned,
bounded public-origin `ResponseCache` (`edgion-gateway/src/response_cache/`,
added in `ef8df076a`, backed by `edgion_algo::cache::ShardedLruCache`; see
[bounded response cache capture](../../http-body/regression-guards/bounded-response-cache-capture.md)).
It captures bodies from Edgion's `upstream_response_body_filter` hook and
abandons on upstream trailers. Still, no manifest declares `pingora-cache` and
`edgion-gateway/src/` never touches Pingora's HTTP cache (`session.cache`;
the only `session.cache` hits are the unrelated Wasm OCI fetch cache), so the
fork harness's Pingora cache-admission contracts still have no subject in
Edgion and the relocation rejection stands on that narrower ground. Whether
Edgion's own cache has equivalent H2 truncated/invalid-trailer non-admission
coverage is outside this page and is not established here. At
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a` the fork target
`pingora-proxy/tests/test_h2_upstream_cache_and_reuse.rs` is unchanged from
`0b0d8bba` and contains 8 `#[tokio::test]` scenarios (the "7 scenarios" count
above is the original record's).

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-proxy-cache-reuse-harness-consumers.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-proxy-cache-reuse-harness-consumers.md)

## Resolution references

- `H2-012` — 2026-08-28 (fixed in pingora fork `edgion_v3`).

## Gateway reuse verification scope

`edgion-h2-upstream-lifecycle-e2e-validation` checks the product's connection
reuse independently of the fork's cache harness. The existing
`upstream_reset_no_error_after_complete_response` transport case requires a
full response while uploading, an actual backend NO_ERROR reset record, a
follow-up on the same upstream connection, and an unchanged accept counter.
The lifecycle `stall_reuse` case additionally requires two window-exhausted
uploads to complete on that same connection after upload cancellation.

These cases use the existing bounded recorder and H1-to-H2/H2-to-H2 paths.
Neither case adds response caching, proves cache admission, or establishes the
absence of every leaked task. Raw backend wire proofs establish the scripted
events; Gateway results establish their product consequences.

Transport's fresh-connection retry controls remain first in registration
order, before lifecycle probes or other shapes warm the Gateway upstream pool.
Changing that order invalidates the fresh-versus-reused retry premise even
when each new lifecycle scenario passes independently.

Historical verification (not repeated after the fork bump): on 2026-10-06
the run used the unchanged Gateway production source at
`e5eadddb43b8d9588a5b8698446dc96f365cae19` and the then-locked fork
`edgion_0.9.0_v1@0b0d8bba9609655bfd7db0fe0add6818bc0ee937`, `h2 0.4.19`.
Cargo.lock now resolves `edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`
(rebased onto upstream 0.9.1) and `h2 0.4.20`; on 2026-11-06 the cited
connector, request-body pump, and END_STREAM watcher sources were re-verified
by source inspection only (see the linked shutdown, writer-stall, and wire
observer pages). The counts and pass results below belong to the `0b0d8bba`
run.
`start_all_with_conf.sh --suites base,HTTPRoute/Basic,HTTPRoute/Transport`
started a fresh shared stack, followed by `test_client -g -r HTTPRoute
-i Transport` with its work-directory/access-log environment. All 165
registered entries passed in 244.65s: 153 executed scenarios and 12 existing
explicit skip entries, with no failures or infrastructure errors. All 20
new lifecycle scenarios executed, including the two supplemental Connector
contracts, and both existing NO_ERROR-reset/reuse protocol combinations passed.
The work directory was `tmp/integration_testing/testing_20261006_181040`.

The outer `run_integration.sh --no-prepare -r HTTPRoute -i Transport` entrypoint
stopped at its macOS loopback-alias preflight, before running tests. Those
aliases serve other LB/GeoIP suites; Transport uses only 127.0.0.1. The passing
result above is the complete standard client suite on the standard shared
stack, not a passing outer-wrapper result. The H2 backend check passed 14 tests,
and the matrix-count unit guard passed. Format, workspace check/Clippy, and all
required project guards passed; independent read-only review returned LGTM.
This resolves the selected coverage task with test-only changes. Production
behavior, dependencies, private watcher races, and response-cache admission
remain outside this verification's changed or freshly tested scope.
