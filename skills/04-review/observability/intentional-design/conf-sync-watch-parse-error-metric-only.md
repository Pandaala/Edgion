---
name: conf-sync-watch-parse-error-metric-only
description: Generic Watch decode diagnostics, scoped acceptance of skip-and-continue, and deferred recovery boundaries.
knowledge_type: intentional-design
---

# Generic Watch decode diagnostics and deferred recovery

## Conclusion and scope

The user explicitly selected the observability-only subset of
`conf-sync-generic-watch-malformed-event-recovery` on 2026-09-07, retaining
generic Watch's skip-and-continue policy while adding safe diagnostics and wire
round-trip coverage. Full recovery is deferred in
[`tasks/todo/conf-sync-generic-watch-malformed-event-recovery.md`](../../../../tasks/todo/conf-sync-generic-watch-malformed-event-recovery.md).
Skipped updates/deletes can leave stale configuration. This is an accepted risk
for this scope, not a disproved defect or a completed recovery fix.

`ClientCache::start_watch` delegates the generic path to
`apply_generic_watch_payload` in
`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs`:

- Invalid top-level event arrays increment the existing
  `edgion_config_event_parse_errors_total{kind}` counter once.
- Each malformed event envelope or typed resource increments it once.
- At most one error is emitted per failed response, identifying the first
  failure using `kind`, `ns`, `name`, `resourceVersion`, `syncVersion`, and
  static error text. Identity comes only from bounded metadata strings; invalid
  or unavailable identity fields are omitted. Top-level failures use response
  `sync_version`; event failures use the event version with the existing
  response-version fallback.
- Raw JSON, specs, and Serde error Display strings are excluded. Serde error
  text can contain the rejected value, including secret material. Logging
  authority: [`03-coding/01-log-safety.md`](../../../03-coding/01-log-safety.md).
- Valid events still apply before and after a rejected event in the same
  response. Readiness, compression, retries, and generation cancellation are
  unchanged. EdgionBackend retains its separate exact-key withdrawal and
  unsafe-batch clearing/relist policy.

The metric keeps only the registered resource `kind` label. Names, namespaces,
versions, and error strings must not become metric labels. No new metric or
consecutive-failure threshold is introduced.

## Rationale and evidence

Shared resource types make successful round trips the expected normal case.
Serialization success alone does not prove deserialization success: mixed builds
and asymmetric/custom Serde implementations can disagree. No measured failure
frequency or production incident was established in this investigation.

Controller `cache_server/cache.rs::watch_json` serializes `Vec<WatcherEvent<T>>`.
The watch sender advances its local `from_version` after sending, without waiting
for Gateway application. Gateway `CacheData.sync_version` is set by List/reset;
it is not a durable applied-Watch cursor. Skipping a bad event does not ask the
same stream to replay it. Metrics and logs improve detection, not convergence.

## Alternatives and limits

The selected scope avoids changing the cache recovery state machine. Complete
batch validation plus terminating the invalid stream and relisting could recover
transient omissions; persistent schema incompatibility may also break List and
requires fixing the producer/consumer contract. A future recovery design must
cover readiness, pending compressed events, backoff, cancellation, and the Backend
exception together. Do not add a local parse-error `break` as a complete fix.

Existing logs are not a substitute for the counter: subsequent bad events in one
response are counted without additional error lines. The first-error summary does
not claim to identify every rejected key. Safe static errors deliberately omit
the decoder's value-bearing explanation.

## Verification and re-evaluation

Verified on 2026-09-07:

- `cargo test -p edgion-gateway --lib conf_sync::cache_client`: 44 passed.
- `cargo fmt --all` and `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with existing warnings;
  warnings in this file concern unchanged List/Backend helpers.
- Agent-doc, SSA-force, metrics-inventory, and Pingora-source guards: passed.
- Independent read-only review: LGTM, including the final test-helper correction.

Inline tests exercise emitted logs and metric counts, mixed valid/invalid batches,
invalid metadata, and Controller-format
add/update/delete events carrying an EdgionTls resolved Secret. This is focused
same-build evidence, not proof for every resource or version pair, nor a live
gRPC or recovery-state-machine test.

Reconsider recovery if operational stale-state evidence makes the accepted risk
unacceptable, if mixed-version support is required, or if the user authorizes the
deferred task. Do not require a production incident to recognize a proven defect.

## Historical evidence and resolution references

- `conf-sync-F013`, closed 2026-05-28: the original metric was added; an
  N-consecutive-failures relist threshold was declined. Historical tests were not
  rerun merely by maintaining this page.
- `conf-sync-generic-watch-malformed-event-recovery`, 2026-09-07: diagnostics
  subset selected; full recovery remains deferred and the source task is retained.
- Earlier wording called schema drift the most common trigger and transient
  corruption vanishingly rare without measured evidence. Those frequency claims
  are not current guidance. Earlier hard-coded cardinality counts are replaced
  by the registered-kind boundary, which includes the Backend-specific consumer.
