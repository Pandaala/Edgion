---
name: center-watch-store
description: Dedicated bounded Center projections, resource-processor publication and generation isolation.
---

# CenterWatchStore

ControllerRuntime owns `fed_sync/center_watch/CenterWatchStore` separately from
the credential-bearing Gateway ServerCache. Its typed records contain only
regional setting identities and RegionRoute consumer identities/references. No
resource spec, routing table, annotations or resolved credentials enter the store.

ResourceProcessor captures its store generation at construction. Both initial
processing and runtime workqueue processing publish after namespace/handler
eligibility checks and before preparse/secret resolution. This makes disabled and
semantically invalid typed objects discoverable without depending on Gateway
publication. These rows describe source configuration; they do not assert reference
availability or Gateway application. Effective detail remains Controller-owned.

Source absence removes the view even when no Gateway cache entry exists.
Eligibility loss and a change away from the projected type also withdraw it.
Gateway `should_publish=false` is not source deletion. All updates stay on the
existing per-kind serial processing path; no second queue or worker-path API IO
is introduced. Runtime reset changes the view generation and rejects late writes,
deletes and failures from old processors.

The store allocates monotonic revisions under its mutex. Snapshot and event
history share that revision boundary. History expiry requires a new snapshot;
it does not silently skip events. Notifications happen after releasing the store
mutex. Receivers must not hold watch borrow guards across store access.

Live state has at most 4,096 resource records and 4 MiB of conservative accounting;
each record is limited to 64 KiB. History has at most 1,024 events and 4 MiB.
The ledger includes escaped-string and retained-capacity headroom. Readers share
record Arcs; the transport must separately bound concurrent snapshot retention.
The plugin projection allows at most 128 entries and 256-byte aliases. These are
publication limits, not changes to the accepted resource schema.

Projection and live-capacity errors are tracked by source key under a 4,096-key /
1 MiB budget. Any outstanding failure blocks successful snapshots and incremental
reads. Other source changes still process. A correction clears its own failure,
including a correction equal to the preserved row; deletion clears failures even
without a retained row. Capacity recovery may requeue the bounded failed-key set
through existing workqueues after space is freed. Workers do not self-retry in a
loop. Exhausting failure tracking, or an unrepresentable source locator, latches
an explicit generation failure rather than pretending incomplete data is complete.

## Federation transport

The reverse Watch dispatcher only accepts the exact view name `RegionRoute` and
requires synthetic `(watch, RegionRoute)` permission. It never resolves generic
CRD kinds through ConfigSyncServer, even with wildcard authorization. Default
policies grant this dedicated Watch instead of generic resource Watch. The access
snapshot exposes `regionRoute.watch` for Center's read-side permission checks.

Every subscription starts with `from_version=0`; nonzero returns RelistRequired
because the request cannot identify its previous store generation. List encodes
`{records:[...]}` and Watch encodes `{changes:[{version,key,record}]}`, where a null
record deletes a key. Protobuf server_id is the CenterWatchStore generation.
Snapshot/version boundaries and history-expiry recovery come from the same store.

Only one producer per federation session can hold a snapshot. It reserves outbound
channel capacity before retaining/serializing data and uses a five-second deadline.
Typed payloads have an 8 MiB wire cap backed by the store's 4 MiB accounting; the
shared 32-message queue can also contain 10 MiB proxy responses, so its total
payload bound is 320 MiB, not a Watch-only estimate. Transport buffers are separate.
Request IDs are nonempty, control-free and at most 128 bytes; rejected oversized
IDs and arbitrary kind strings are dropped before spawning tasks or logging.

Permissions/readiness/generation are checked before data sends and every second
while idle. Error/history expiry/reset ends the subscription; failure to send its
terminal error cancels the federation session. Waiting for capacity never bypasses
a later permission or generation check. New List attempts can requeue the bounded
failed-key set through existing processors; resource workers do not self-retry.

Center now validates the typed wire contract, isolates receiver sessions and
requests, and freshly authorizes each directory read, including owner-forwarded
reads. The browser consumes the bounded directory and fetches source details on
demand. Controller cannot retract already queued frames. Cross-repository
acceptance and final independent review verified the ISSUE-009 boundary. The
maintained [federation regression guard](../../04-review/federation/regression-guards/fed-proxy-request-sanitisation.md#processed-resource-boundary-issue-009)
records its root cause, credential boundary and re-evaluation triggers.
