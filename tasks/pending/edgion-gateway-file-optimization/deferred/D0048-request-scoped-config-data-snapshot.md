# D0048 — Use One EdgionConfigData Snapshot per Authorization Decision

## Meta
- Status: resolved
- Severity: high
- Discovered in: B0082, IpRestriction audit
- Category: concurrency-correctness
- Closed: 2026-09-08
- Canonical record: [`skills/04-review/plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md`](../../../../skills/04-review/plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md)

## Evidence
Each `get_ip_list_matcher_by_key` call loads the global ArcSwap independently, and deny and allow references are resolved in separate calls. One HTTP request or stream/TLS connection can therefore combine matchers from different published EdgionConfigData generations. GlobalConnectionIpRestriction can additionally load its Selector separately from the IpLists it references.

## Resolution
B0123 added one canonical owned snapshot handle and migrated HTTP IpRestriction, stream IpRestriction, TLS-route restriction, and GlobalConnectionIpRestriction. Each authorization decision now resolves every referenced matcher, and GCIR's Selector, from one immutable generation. Reference-free decisions retain their zero-store-load fast path. A deterministic two-generation regression proves that both complete generations deny while the formerly possible torn tuple allows.

## Integration-test impact
No integration-test issue is required. Store-level generation tests and deterministic consumer tests can prove that every key used by one decision comes from one immutable snapshot.
