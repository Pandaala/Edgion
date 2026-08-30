# TLS store true incremental and atomic publication

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Replace the TLS store's fake partial update with a genuine per-key matcher update and publish certificate data plus its derived listener matcher as one coherent generation. The difficult part is preserving deterministic host-collision precedence, last-known-good certificate retention, and lock-free handshake reads without rebuilding or cloning the entire store for every delta.

## Confirmed current limitations

- `TlsStore::apply_partial_update` clones the complete `HashMap<String, TlsEntry>` and calls `rebuild_matcher_from_data`, so its delta-shaped API remains O(total certificates and hosts).
- `rebuild_matcher_from_data` publishes the global matcher before `tls_data` is published. Readers can briefly observe certificate statistics/validation from one generation and SNI selection from another.
- Collision warnings and last-known-good transition state are derived during the full rebuild, so they need explicit ownership in a true incremental design.

## Requirements

- Apply add, update, and remove deltas without cloning every unchanged `TlsEntry` or rebuilding every listener matcher.
- Preserve deterministic EdgionTls precedence: oldest `creationTimestamp`, then namespace, then name.
- Preserve last-known-good behavior for invalid replacements, including rejection transition re-arming after a valid update.
- Publish the certificate entries and every derived listener/host matcher as one immutable generation visible through one lock-free snapshot read.
- Keep configuration processing synchronous and free of network calls, timers, sleeps, or additional worker queues.
- Do not introduce a third in-process cache implementation.

## Scope

### In scope

- `edgion-gateway/src/tls/store/tls_store.rs`
- `edgion-gateway/src/tls/store/cert_matcher.rs`
- TLS store and matcher unit tests plus cache-residue accounting if storage shape changes.

### Out of scope

- Certificate validation semantics.
- Controller workqueue behavior or conf-sync transport changes.
- Gateway rustls support or parity.

## Acceptance criteria

- A one-key delta has work proportional to the changed key's affected listeners/hosts, not the complete store.
- Unchanged `Arc<EdgionTls>` entries and unaffected listener matchers retain pointer identity across a delta.
- Readers cannot observe mixed generations between validation/status data and SNI matching.
- Delete, delete-to-create coalescing, invalid replacement, valid recovery, hostname change, listener change, and collision-winner change have focused tests.
- Full replacement still removes absent keys and retains a last-known-good entry only for an invalid incoming value with the same key.
- Collision logging remains transition-bounded and does not enumerate unbounded resource or hostname lists.
- Targeted TLS tests, workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/04-tls/SKILL.md`
- `skills/01-architecture/02-gateway/04-tls/01-downstream-tls.md`
- `skills/01-architecture/00-common/05-unification-points.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-30 | Defer implementation from the file audit. | Correctness requires a unified immutable generation and per-listener reverse indexes; a local delta patch would retain either O(n) cloning or mixed-generation publication. |
