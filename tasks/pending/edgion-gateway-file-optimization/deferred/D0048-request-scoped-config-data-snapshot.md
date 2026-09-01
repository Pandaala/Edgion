# D0048 — Use One EdgionConfigData Snapshot per Authorization Decision

## Meta
- Status: needs-discussion
- Severity: high
- Discovered in: B0082, IpRestriction audit
- Category: concurrency-correctness
- Canonical task: [`tasks/todo/request-scoped-edgion-config-data-snapshot.md`](../../../todo/request-scoped-edgion-config-data-snapshot.md)

## Evidence
Each `get_ip_list_matcher_by_key` call loads the global ArcSwap independently, and deny and allow references are resolved in separate calls. One HTTP request or stream/TLS connection can therefore combine matchers from different published EdgionConfigData generations. GlobalConnectionIpRestriction can additionally load its Selector separately from the IpLists it references.

## Why deferred
Correctness requires one canonical snapshot API and coordinated migration of HTTP IpRestriction, stream IpRestriction, TLS-route restriction, and GlobalConnectionIpRestriction. An HTTP-only cached load would leave other consumers inconsistent and create a second access pattern.

## Integration-test impact
No integration-test issue is required. Store-level generation tests and deterministic consumer tests can prove that every key used by one decision comes from one immutable snapshot.
