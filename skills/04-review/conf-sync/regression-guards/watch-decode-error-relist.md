---
name: watch-decode-error-relist
description: Generic Watch decoding stops at the first failure and recovers through List without rolling back valid events.
knowledge_type: regression-guards
---

# Generic Watch decode failure recovery

## Conclusion and scope

`ClientCache::start_watch` in
`edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs` ends the current
generic Watch when `apply_generic_watch_payload` reports a decode failure.
The existing outer loop backs off, obtains a valid List snapshot, resets the
cache, and only then starts another Watch. No newer response from the abandoned
stream is applied. A top-level JSON failure changes no cache entries; an envelope
or typed-resource failure retains the successfully applied prefix and leaves the
remaining events for the replacement List.

The user selected this minimal recovery after comparing client-go behavior.
The task's original whole-batch rollback requirement was a proposed design,
not a product invariant, and was explicitly narrowed. There is no new transaction,
applied-version cursor, readiness switch, or event generation mechanism.

## Rationale and evidence

The Controller sender advances watcher-local `from_version` after sending,
without waiting for Gateway application. Continuing after a skipped event could
therefore leave a key stale indefinitely. Gateway `CacheData.sync_version` is
set by List/reset; it is not an applied-Watch cursor. Ending the stream reuses the
existing List-before-Watch recovery rather than inventing cursor replay.

`CacheData.data` contains the latest successfully decoded resources.
`handler.compressed_events` contains pending keys and change histories, not
another resource-body copy. Its worker waits for a notification and a 100ms
window, then invokes `partial_update` under the cache write lock. `reset`
invokes `full_set` and clears the pending queue under the same lock. A valid
prefix may publish before reset; pending old events cannot publish after reset.
This proves cache callback ordering, not atomic completion of every downstream
asynchronous operation.

Generic readiness remains true after a successful initial List even while
recovery is pending: it represents available configuration, not current server
freshness. Initial List failure remains unready. A failed generic replacement
List retains the existing cache, runtime configuration, and pending valid events.
EdgionBackend keeps its stricter exact-key withdrawal and unsafe-batch clearing
policy. Reload-generation cancellation and retry/backoff remain unchanged.

## Diagnostics and safety

The existing `edgion_config_event_parse_errors_total{kind}` counts the first
decode failure terminating each generic Watch. The unprocessed tail is not
decoded or counted. One error carries bounded `ns`, `name`,
`resourceVersion`, trusted `kind`, event `syncVersion` (response fallback),
and static error text. Top-level failures have no resource identity.
Raw payloads and value-bearing Serde Display strings are excluded, including
List decoder errors reached during recovery. No new metrics or labels are added.
Authority: [log safety](../../../03-coding/01-log-safety.md).

The Gateway API List/Watch boundary additionally checks raw `apiVersion` and
`kind` against the canonical source inventory before typed deserialization.
A beta or malformed TypeMeta in a generic Watch is a decode failure and follows
this same relist path; a List with such an item is rejected before publication.
Focused 2026-09-24 tests cover missing/malformed/beta List TypeMeta and a
beta Watch event followed by canonical recovery. This does not add a Gateway
source-version converter or change the existing successful-prefix policy.

## Alternatives and limits

Persistent producer/consumer schema mismatch can also prevent List from
succeeding. Old configuration, including old authorization settings, may remain
active until the mismatch is corrected; this is not fail-close. The existing
backoff limits retries and readiness is not withdrawn solely for this condition.

Ordinary client-go Watch applies individual events without rolling back an
already applied prefix, and its StreamWatcher terminates on decoder failure.
Reflector may resume Watch or return to List depending on the error; this does
not require Edgion to add version-resume machinery. Informer initial-sync state
is not an application readiness guarantee. Sources consulted during design:
[StreamWatcher](https://github.com/kubernetes/apimachinery/blob/master/pkg/watch/streamwatcher.go),
[Reflector](https://github.com/kubernetes/client-go/blob/master/tools/cache/reflector.go),
[DeltaFIFO](https://github.com/kubernetes/client-go/blob/master/tools/cache/delta_fifo.go).
These master-branch references are design evidence, not pinned dependency contracts.

## Verification and re-evaluation

Current recovery verification (2026-09-07):

- `cargo test -p edgion-gateway --lib conf_sync::cache_client`: 45 passed.
- `cargo fmt --all`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with existing warnings.
- Agent-doc, SSA-force, metrics-inventory, and Pingora-source guards: passed.
- Independent read-only code review: LGTM.

Focused tests cover first-failure stopping, preserved valid prefixes, safe
diagnostics, valid Controller-format TLS Secret round trips, failed List
retention, both compressed-event/reset callback orders, and resumed publication.
The full live gRPC reconnect lifecycle is established by the outer-loop control
flow and independent review, not exercised by these in-process unit tests.
Existing Backend withdrawal and generation-backoff tests remain required.

Re-evaluate if batch transactions become an explicit product requirement,
cache callbacks become asynchronous, reset no longer clears the pending queue
under the same lock, or service readiness must reflect configuration freshness.

## Historical evidence and resolution references

- `conf-sync-F013`, 2026-05-28: metric-only handling and an accepted decision
  against an N-consecutive-failures threshold. The skip-and-continue part is
  superseded by immediate first-failure recovery.
- `conf-sync-generic-watch-malformed-event-recovery`, 2026-09-07: initially
  narrowed to observability (44 cache-client tests and guards passed then);
  subsequently authorized as minimal first-error List/Watch recovery with valid
  prefixes retained. Earlier checks do not establish verification of this fix.
- This page rewrites and relocates the existing observability decision rather
  than creating a duplicate finding archive. No measured failure frequency or
  production incident is claimed.
