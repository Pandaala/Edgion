---
name: h2-proxy-cache-reuse-harness-consumers
description: Historical review evidence for Pingora fork / H2 proxy cache-admission and upstream reuse; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 proxy cache reuse harness consumers

## Recorded conclusion

The H2 audit's proxy test harness (cache entry inspection, injected H2 windows, local response-body failure, upstream socket observability) had no end-to-end callers, so cache admission, connection reuse and local-failure isolation were unproven. Fixed by the new standalone target `pingora-proxy/tests/test_h2_upstream_cache_and_reuse.rs` (7 scenarios) asserting on cache state, origin socket identity, and the upstream handshake windows rather than downstream keepalive. Adding a dedicated CI job, relocating the coverage into `edgion-tests` (the Edgion Gateway implements no HTTP response cache — no manifest declares `pingora-cache` and `edgion-gateway/src/` never touches `session.cache` — so cache admission never runs and the contracts have no subject there), asserting reuse from keepalive alone, asserting the connection window through a behavioural flow-control scenario (it is pinned at the handshake instead, and any value at or below 65535 is a no-op), and claiming the reuse assertion proves no pump task leaked are not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-proxy-cache-reuse-harness-consumers.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-proxy-cache-reuse-harness-consumers.md)

## Resolution references

- `H2-012` — 2026-08-28 (fixed in pingora fork `edgion_v3`).
