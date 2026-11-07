---
name: 02-cache-residue-detection
description: Cache residue detection — wipe-all + store-stats based leak detection at end of integration runs. Use when adding new in-memory stores, debugging leak reports, or extending the validator.
---

# Cache Residue Detection

> After all scenarios in `run_integration.sh` finish, the controller's `wipe-all` is invoked to clear every resource, and then we assert that all in-memory cache counts are 0. Any residue is treated as a **leak**.

## Workflow

1. `run_integration.sh` invokes the `cache_residue` binary after all test loops finish and before `cleanup`.
2. `cache_residue` calls `POST controller /api/v1/admin/wipe-all` (registered only in testing mode), which deletes all 22 kinds in tier order.
3. It calls `DELETE gateway /api/v1/testing/access-logs` to clear the access log (eliminates TTL false positives).
4. It polls (default 500ms interval, up to 75s) to allow resource propagation,
   background cancellation, and outstanding leases to settle. Wasm runtime
   destruction has no intentional eviction grace:
   - `GET controller /api/v1/debug/store-stats`
   - `GET gateway /api/v1/debug/store-stats`
   - The built-in ignore-path filters out structural constant fields (e.g., `next_resource_version`).
5. All zero → exit 0; residue still present after timeout → exit 1 (mode=fail) / 0 + warning (mode=warn); endpoint unreachable → exit 2.

## Adding a new store to leak detection

When adding any in-memory store / cache, it must be wired into store-stats, otherwise `cache_residue` cannot detect a leak in that store.

### Gateway side

1. Add `pub fn count(&self) -> usize` and `pub fn keys(&self) -> Vec<String>` to the store.
2. Add a field in `StoreStatsResponse` of `edgion-gateway/src/api/mod.rs` and populate it in the `store_stats()` handler.
3. Add a unit test: `empty store reports zero`.

### Controller side

1. Add the same methods on the store.
2. Add a field in `ControllerStoreStats` of `edgion-controller/src/api/debug_handlers.rs` and populate it.
3. Add a unit test.

ConfCenter resource counts are derived from `ALL_RESOURCE_INFOS`; never add a second kind list to
the store-stats handler. The wipe-tier coverage test compares its tier union with the same canonical
inventory, so adding a resource fails the test until its deletion order is chosen.

### When **not** to add it

- The store has the same lifecycle as a parent object (e.g., a cache inside a plugin instance, a private map of a route handler): the whole thing is reclaimed when the parent is dropped via ArcSwap; there is no independent leak signal. Leakage is exposed by the parent's store-stats.
- TTL-based store (e.g., access log): a clear API already exists before cache_residue is called.

## Metrics lifetime state

The canonical Common metrics recorder deliberately retains its lifetime admission
ledger and initialized fixed tuples. Gateway dynamic event epochs can remain for
600 seconds, longer than the default 75-second residue poll; Controller fixed
state also persists until process shutdown. This persistence is intentional and
must not be classified as a removable business cache.

Both binaries expose `metrics.admission_count`, `fixed_initialized_count`,
`live_epochs`, `key_bytes`, `overdue_epochs`, `gauge_owners` and
`attached_gauge_owners`. The first four exact numeric paths are exempt from
zero-after-wipe. Admission remains subject to the configured lifetime soft cap
plus valid fixed tuples (observe mode and cap zero do not promise a memory cap).
Key sizes are bounded at descriptor preparation. Diagnostics sweep expiration
through the same canonical per-slot eviction path as scraping.

`overdue_epochs`, `gauge_owners` and `attached_gauge_owners` must all reach zero.
Never ignore these paths or the entire `metrics` object. Nonexpired event history
is legitimate; resource-owned gauge contribution tokens must release after wipe.
The dedicated MetricsLifetime scenario uses a two-second TTL to prove actual
idle removal and later renewal. The default untargeted integration runner invokes
`test_metrics_lifetime.sh` through its bounded isolated-scenario queue before the
shared topology starts. Its lifetime cap of 64 deliberately saturates admission,
so it owns a separate filesystem Controller, Gateway and small HTTP backend;
sharing the ordinary Gateway would pollute later suites' lifetime metrics state.
The original MetricsLifetime fixtures and 120-second phase assertions are reused.
Ports are registered and preflighted even for direct invocation; an existing
service causes failure rather than being stopped. Owned binaries honor artifact
selection and coverage instrumentation. Cleanup signals only recorded child PIDs,
stops the Gateway before its sync source, and allows the coverage Gateway its
60-second profile-flush window. Logs remain in the owned integration work directory.
The capability guard requires the real default mapping and failure-propagating
scenario invocation; targeted-only registration does not establish default coverage.
Unit tests cover the cached route matrix directly
because integration mode uses the logging fallback path. Reverse verification
must independently break expiration and owner release and confirm that these
exact residue paths fail; restore both faults before final verification.

## DNS answer cache entries

The DNS answer caches (`gateway.dns_answer_cache.{upstream,egress,request_egress}`,
`controller.dns_answer_cache`) retain capacity-bounded answers up to their
5-minute stale limit plus one 30 s stale window (at most 630 seconds after a
300 s record TTL), beyond the 75-second poll, and carry no resource identity,
so wipe-all cannot release them. `request_egress` lists no flight keys.
Their exact `entries` paths are exempt. `in_flight` (live single-flight
registrations) must reach zero; `in_flight_keys` names any stuck key.

## Adding a new ignore-path

An ignore-path is a structural constant (a legitimate value that never returns to zero), such as:
- Monotonically increasing sequence counters
- A fixed number of registered items (kind list, watch object count)
- Values that are normally `>0` during the test (e.g., `gateway_config.gateways` — sourced from `GatewayStore::count()` since runtime-F003)

Modify the `IGNORE_PATHS` constant in `edgion-tests/src/validator/cache_residue.rs`. Every addition must include a commit message explaining why this field does not count as a leak.

## Reverse verification

Any extension to store-stats or wipe-all must be reverse-tested manually: temporarily short-circuit the delete path → run `run_integration.sh` → verify that the cache_residue report precisely locates the issue. See spec §6.2.

## Debugging a leak report

Example report:

```
[FAIL] Cache residue detected after wipe-all (timeout 75s):

  controller:
    conf_center_storage.HTTPRoute        = 1
    secret_store                          = 2
  gateway:
    client_caches.by_kind.Service         = 1   sample: ["default/svc-x"]
    plugin_store_v2.by_kind.JwtAuth       = 1   sample: ["default/jwt-a"]

Total residue: 5 entries across 4 buckets
```

Diagnosis order:
1. Residue in `controller.conf_center_storage.<Kind>` → wipe-all missed that kind; check the TIER list in `wipe_handlers.rs`
2. Residue in `gateway.client_caches.by_kind.<Kind>` → controller→gateway sync did not converge (raise `--timeout-secs`), or there is a bug in the gateway ConfigClient delete path
3. Residue in `gateway.<store_name>.*` → the store's delete handler does not properly remove entries; trace the code by the sample key
4. Residue in `gateway.plugin_store_v2.by_kind.<Kind>` → PluginStore's conf_handler does not respond to the delete event

## Related links

- Spec: `docs/superpowers/specs/2026-04-26-cache-residue-detection-design.md`
- Plan: `docs/superpowers/plans/2026-04-26-cache-residue-detection.md`
- Validator: `edgion-tests/src/validator/cache_residue.rs`
- Wipe handler: `edgion-controller/src/api/wipe_handlers.rs`
- Gateway store-stats: `edgion-gateway/src/api/mod.rs:312-442`
- Controller store-stats: `edgion-controller/src/api/debug_handlers.rs`
