---
name: ring-buffer-fixed-capacity-toooldversion-relist
description: Use when reviewing findings that compare Edgion against Envoy/Istio and claim "Controller-side EventStore is a fixed-capacity per-kind ring buffer / high-churn kinds (EndpointSlice, Service) silently lose events / TooOldVersion forces an expensive full relist / thundering-herd full Lists during a rolling deploy / no per-kind capacity setting / no Delta-xDS-style explicit add-remove without ring-buffer expiry". Covers why the configurable ring buffer + TooOldVersion→relist design is an accepted tradeoff (per-kind capacity IS configurable; event-compaction is a large feature, not a narrow fix; ring-buffer utilization metrics are a separate observability enhancement).
status: accepted-tradeoff
finding_id: sync-06
closed: 2026-06-14
---

# Fixed-capacity per-kind ring buffer — TooOldVersion forces a full relist — ACCEPTED-TRADEOFF (capacity is configurable; compaction is a large feature)

## Conclusion

The Controller-side `EventStore<T>` is a fixed-capacity circular ring buffer per
resource kind (`edgion-controller/src/conf_sync/cache_server/store.rs`). When it
wraps, the oldest event is evicted and `expire_sync_version` advances; a Gateway
whose `from_version` falls below `expire_sync_version` receives
`WATCH_ERR_TOO_OLD_VERSION` and falls back to a full List. This is an **accepted
design tradeoff**, not a defect, because per-kind capacity is **operator-tunable
via config** (the escape valve already exists), the full-List fallback is bounded
by a hard response-size limit, and the only treatments left — a Delta-xDS-style
event-compaction model, and ring-buffer-utilization metrics — are respectively a
large cross-layer feature and a separate observability enhancement, neither of
which is a narrow review fix.

Fix suggestions of the form "add a per-kind capacity setting" (already exists),
"replace the ring buffer with explicit per-resource add/remove and no expiry like
Delta xDS / maintain a compacted current-state + change journal" (large feature),
or "treat the fixed default as a regression vs Envoy" are **not accepted** as a
narrow review fix.

## Core Rationale

**1. Per-kind capacity is already configurable — the original "no per-kind setting" claim is false**

`ConfSyncConfig` (`edgion-common/src/conf_sync/config.rs:21-67`) exposes
`default_capacity` (global default, 200) and `capacity_overrides` (per-kind
override map). `get_capacity(kind)` resolves the override first, then the global
default. So an operator can raise EndpointSlice/Service capacity for a large or
high-churn cluster **without recompiling** — the escape valve the finding asks
for already ships. The finding's own second-pass review retracts the original
"there is no per-kind capacity setting" sub-point; what remains is that the
uniform 200 default is not documented as insufficient for high-churn kinds.

**2. The TooOldVersion → full-List fallback is intentional and bounded**

Falling back to a full List on `TooOldVersion` is the deliberate
correctness-over-bandwidth choice: a Gateway that has provably missed events
re-establishes ground truth rather than applying a gapped delta. The fallback is
bounded by the 64 MB hard limit on List responses
(`conf_server/config_sync_server.rs:147-170`), which exists precisely because a full
EndpointSlice List can be large. This is the same "full resync is authoritative"
posture accepted across the config-sync family (see Reference Cases), not a bug.

**3. Event compaction is a large cross-layer feature, not a narrow fix**

The finding's medium-term direction — move high-churn kinds from a raw-event ring
buffer to a compacted "current state per resource key + small change journal" so a
far-behind Gateway gets only the current state of the keys it watches (the Delta
xDS model) — touches the server-side store data structure, the resume/diff
protocol over the Watch stream, and the Gateway-side apply path. That is
medium-risk cross-layer work (state-machine + protocol change, cross-kind
consistency) and belongs in `tasks/todo/` via brainstorming if ever prioritized —
exactly the disposition applied to the sibling sync findings.

**4. Ring-buffer-utilization metrics are a separate observability enhancement**

The finding's short-term "add metrics for `expire_sync_version` advancement rate
and per-kind `TooOldVersion` error rate" is a legitimate observability ask, but it
is an independent enhancement to the (cold-path) conf-sync server, not a fix for
this design "defect." It can be filed on its own merits; it does not gate closing
this finding, and bundling new metrics plumbing into a low-severity Envoy-parity
finding would exceed the narrow review-fix scope.

## Fix Suggestions Not Accepted

- "Add a per-kind capacity setting so high-churn kinds get a larger buffer" —
  already exists as `conf_sync.capacity_overrides` + `default_capacity`
  (`config.rs:21-67`); re-reports premised on its absence are factually wrong.
- "Replace the ring buffer with Delta-xDS-style explicit per-resource add/remove
  and no event expiry / maintain a compacted current-state + change journal" —
  large cross-layer feature (store + Watch protocol + Gateway apply path),
  medium risk; promote to `tasks/todo/` via brainstorming, not a narrow fix.
- "Add ring-buffer-utilization / TooOldVersion-rate metrics as part of this fix" —
  a worthwhile but independent observability enhancement; file separately, it does
  not belong bundled into this finding.
- "Treat the uniform 200 default / fixed ring buffer as a regression vs Envoy" —
  rejected on the facts: capacity is tunable per kind, the full-List fallback is
  the intentional authoritative-resync posture, and Envoy's Delta xDS is a
  different protocol model, not a bug bar Edgion fails.

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows that even with tuned `capacity_overrides`, a rolling
  deploy still drives a fleet-wide `TooOldVersion` thundering-herd of full Lists
  that measurably overloads the Controller — i.e. the escape valve proves
  insufficient in practice rather than merely undocumented.
- Ring-buffer-utilization / `TooOldVersion`-rate metrics become a hard operator
  requirement (then file the observability enhancement on its own).
- The project moves config-sync to an incremental/compacted resume model for an
  unrelated reason, making event compaction an incremental rather than greenfield
  change.

## Reference Cases

- `tasks/review-envoy/sync-06-ring-buffer-capacity-is-per-kind-fixed.md` (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `edgion-controller/src/conf_sync/cache_server/store.rs:177-201` (`mut_update` eviction + `expire_sync_version`; `get_events_from_sync_version` TooOldVersion gate)
- `edgion-common/src/conf_sync/config.rs:21-67` (`ConfSyncConfig.default_capacity` + `capacity_overrides`, `get_capacity` resolution)
- `edgion-controller/src/conf_sync/conf_server/config_sync_server.rs:147-170` (`MAX_LIST_BYTES` 64 MB List response hard limit bounding the relist)
- `skills/04-review/architecture/gateway-no-local-config-snapshot.md` (sibling config-sync Envoy-comparison tradeoff, sync-05 — "full resync is authoritative")
- `skills/04-review/architecture/no-ack-nack-control-plane-validation.md` (sibling config-sync Envoy-comparison tradeoff, sync-01)
- `skills/04-review/concurrency/watcher-send-await-backpressure-no-timeout.md` (sibling sync-03; the slow-client stall this finding's feedback-loop point 3 references)
