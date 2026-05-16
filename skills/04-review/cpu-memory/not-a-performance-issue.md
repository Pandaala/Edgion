# Common CPU / Memory False-Positive Scenarios

> Source: 2026-03-30 full-project CPU/Memory deep review (574 Rust source files); conclusions verified one-by-one against source code.

---

## metrics label `.to_string()` is an API Hard Requirement

**False-positive scenario**: criticizing `.to_string()` in `record_backend_request()` / `record_request_duration()` as causing unnecessary heap allocation per call.

**Reality**: `metrics` 0.24's label value type is `SharedString = Cow<'static, str>`. Dynamic values (gateway name, namespace, route, etc., which are not `'static`) **must** be `Cow::Owned(String)`; `.to_string()` is the minimal API-satisfying form. The `Arc<str>` and `&str` approaches are infeasible because their lifetimes do not satisfy `'static`. The main cost of metrics calls comes from registry hash + atomic operations, not the string allocation itself.

**Verdict**: not a performance issue; no fix needed.

---

## `get_raw_any` / `lookup_gateway` is Not JSON Serialization

**False-positive scenario**: thinking that `lookup_gateway()`, `lookup_service_cluster_ip()` call `list_json()` to fully serialize all entries to JSON and then linearly scan.

**Reality**:
```rust
// route_utils.rs
pub fn lookup_gateway(namespace: &str, name: &str) -> Option<Arc<Gateway>> {
    let processor = PROCESSOR_REGISTRY.get("Gateway")?;
    let key = format!("{}/{}", namespace, name);
    processor.get_raw_any(&key)?.downcast::<Gateway>().ok()
}
```
`get_raw_any` goes through `KubeRawStore` (DashMap) or `FsRawStore` (RwLock<HashMap>), both **O(1) lookups**. `downcast` only does a TypeId comparison, no JSON serialization, no linear scan.

**Verdict**: code is correct and efficient; not a performance issue.

---

## HTTP/gRPC Route Regex Matching: Controller Validates + No Fallback on Hot Path

**Historical false positive**: in `match_header` / `match_query_param`, there used to be a fallback branch that called `Regex::new()` when `compiled_regex` was `None`; the audit repeatedly flagged it as a hot-path performance issue.

**Defense at the time**: claimed "the route is uniformly precompiled at creation; the fallback branch only fires on a config bug; normal runtime does not trigger".

**Flaw in the defense**: at the time, the Controller-side `validate_http_route_if_enabled` / `validate_grpc_route_if_enabled` **only validated the `match_type` enum value**; it did not actually try `Regex::new()`. Illegal regex could pass through to the data plane unchecked; `compile_match_regexes` used `Regex::new(...).ok()` to silently swallow the error; ultimately every request would land on the fallback and recompile + refail — both hiding the config error and paying hot-path cost.

**Current fix (2026-04-25)**:
1. On the Controller side, `ref_grant/validator.rs::check_regex_pattern` is now invoked inside `validate_*_route_if_enabled` to try `Regex::new()` for all `RegularExpression` fields (HTTPRoute path/header/query, GRPCRoute method.service/method.method/header); failures are pushed to errors → route `Accepted=False`.
2. On the Gateway side, `match_unit.rs::match_header` / `match_query_param` (HTTP & gRPC) **runtime fallback removed**; `compiled_regex == None` returns `Ok(false)` directly, fail-closed.
3. On the Gateway side, `compile_match_regexes` still keeps `Regex::new(...).ok()` (defense against controller bugs); on failure it loudly alerts via `tracing::error!`, not silent.

**Decision criteria during review**:
- See no `Regex::new` call inside `match_header` / `match_query_param` → already in the post-fix state, not an issue.
- See `Ok/Err` two-arm with `tracing::error!` inside `compile_match_regexes` → this is the data plane's last line of defense against controller bugs, not a performance issue.
- See controller `check_regex_pattern` calls → indicates illegal regex is rejected at admission; the Gateway fallback path is theoretically unreachable.

**Verdict**: in the current implementation the fallback has been completely removed; any "suggest adding runtime Regex::new to match_header" finding is a reverse regression.

---

## Watcher Vec Lazy Cleanup is Sufficient

**False-positive scenario**: `watchers: RwLock<Vec<WatchClient<T>>>` does not shrink immediately after a disconnection; flagged as memory growth.

**Reality**: each new watcher registration runs `retain(|w| !w.sender.is_closed())` to clean disconnected old watchers; cleanup is timely as new and old watchers alternate. In the Controller scenario, watcher counts are limited (typically < 100); even if all accumulate it does not exceed 20KB. Adding retain to `notify_watchers` would acquire the write lock on every broadcast, instead increasing hot-path lock contention.

**Verdict**: lazy cleanup is sufficient; the current design is reasonable; not a memory issue.

---

## Webhook Read-Then-Truncate Has a 16KB Hard Cap

**False-positive scenario**: `read_body_limited` calls `resp.bytes().await` to read fully into memory then truncate; described as "1GB full buffering".

**Reality**:
```rust
// Call site
let max_bytes = runtime.config.max_response_bytes.min(WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES);
pub const WEBHOOK_GLOBAL_MAX_RESPONSE_BYTES: usize = 16384;  // 16KB hard cap
```
Two layers of `.min()` guard; worst case extra memory read and discarded = 16KB, not the 1GB described. The code comment already acknowledges this limit and notes "sufficient since max_bytes is capped at 16KB".

**Verdict**: impact is minimal (extra 16KB); no fix needed.

---

## OnceLock Global Singleton is Deliberate Design

**False-positive scenario**: `GLOBAL_CONFIG_CLIENT: OnceLock<Arc<ConfigClient>>` cannot be released; flagged as "hot reconnect cannot reclaim the old ConfigClient".

**Reality**: within the Gateway process lifetime, the Controller endpoint connected to is fixed; **hot-switching the Controller is not a current feature requirement**. The `OnceLock` global singleton is a deliberate design decision. If hot reconnect is needed in the future, the entire config-sync architecture needs to be redesigned systematically rather than simply replaced with mutable state (which would introduce a host of concurrency-safety issues).

**Verdict**: deliberate design; not a memory issue.

---

## Small-Object Clones on Resource Reconcile / Config Sync Paths Are Not Issues

**False-positive scenario**: on "config-change-triggered" paths such as Controller `resource_processor` / Gateway `conf_handler`, seeing `.clone()` / `.to_vec()` / full HashMap copies on a single resource, single entry, single service is flagged as "P3 unnecessary clone / should switch to Arc<T>". Typical examples:

| Location | Pattern | Trigger frequency |
|------|------|---------|
| `handlers/secret.rs` clones the same secret once for init_accumulator and once for secret_store | Secret resource reconcile | Per Secret create/update |
| `lb/lb_policy/policy_store.rs::get(service_key) -> Vec<LbPolicy>` | DashMap entry policies `.to_vec()` | HTTPRoute reconcile |
| `plugins/runtime/plugin_runtime.rs` `add_from_httproute_filters_inner` / `create_request_filter_from_edgion` / `create_upstream_response_filter_from_edgion` calling `XxxFilter::new(config.clone())` | Inputs are `&[HTTPRouteFilter]` / `&EdgionPlugin`; filter constructors take config by value | PluginRuntime build (reload / route change) |

**Reality**: these clones are all on reconcile / config-sync paths; call frequency = K8s resource change frequency (human/operator scale, with bursts at most at reload time); **not per-request**. The cloned objects are usually single resources (KB scale); the entire path is dominated by etcd RTT / gRPC serialization; a few String/HashMap clones' alloc + memcpy are immeasurable in totals.

To switch them to Arc one usually needs:
- Modifying public type signatures (`WorkItemResult::obj`, `PolicyStore::get` return type, etc.), cascading to many callers/tests;
- Or adding closure APIs (`with_policies(&self, key, F)`) that change upper-layer usage.

No observable benefit on the cold path, but real change surface and regression risk.

Special note on the plugin_runtime line (added 2026-05-08): audits often misread this as "eliminate clone"; in reality:
- The current call-chain shape is `&[HTTPRouteFilter]` / `&EdgionPlugin` → `match` to `&Config` → `XxxFilter::new(config: OwnedConfig)`. To get owned `T` from `&T` you **must** clone.
- Moving the clone into the constructor (changing it to `new(&Config)` with internal clone) is just relocation; the alloc count is unchanged.
- To truly eliminate the clone, the entire call chain must be passed by value (`Vec<HTTPRouteFilter>` / `Vec<RequestFilterEntry>` / `EdgionPlugin`), cascading to `RouteRuntime` construction, hot-update logic, and write-style changes for all `WrongStage` branches still needing to borrow plugin to get name — impact far exceeds the "local clone optimization" label.
- Switching to `Arc<Config>` would touch every plugin struct/trait; not worthwhile.
- Trigger frequency is reload / route change only; not the request path.

**Verdict**: whether on the hot path decides everything—
- ✅ Reject: "P3 small-object clones on reconcile / config sync paths" findings.
- ✅ Reject: "plugin_runtime build-time config.clone()" findings (API shape decides; relocation does not save; full-chain by-value change is too large).
- ⚠️ Accept: "full_set whole map copy and synced per second" amplification scenarios (existing cases: LinkSys `replace_all` uses `Arc::clone` to share with the dispatch task, avoiding whole HashMap copy; ACME `challenge_store::lookup` switched to `Bytes`, saving one String alloc + memcpy per HTTP-01 verification request).
- Acceptance bar: the finding must prove either "triggered per second on the request path" or "single copy ≥ MB scale"; otherwise close per this entry.

---

## OIDC / JWT Claims JSON Serialization is the Opt-In Feature Itself

**False-positive scenario**: criticizing `openid_connect/userinfo.rs:60-74` and `jwt_auth/verify.rs:172-176` for doing `serde_json::to_string(claims)` per authenticated request; suggesting "lazy serialization" or "documentation warning that it is enabled by default".

**Reality**:

The three relevant switches all default to `false`:
- `set_userinfo_header` → `openid_connect.rs:340` = `false`
- OIDC `store_claims_in_ctx` → `openid_connect.rs:343` = `false`
- JWT `store_claims_in_ctx` → `jwt_auth.rs:187` = `false`

"Users very easily turn it on by default" does not hold. Once a switch is on, serialization is the feature itself:

- The `X-Userinfo` header must be written before forwarding to upstream; downstream has no trigger point for "lazification".
- The `set_ctx_var(name, value)` signature is `&str`; the ctx-var model stores owned String. To do "serialize on read", the ctx var must be changed to a typed `Value`, affecting the entire plugin runtime ctx interface.
- Claims are typically < 1KB; `serde_json::to_string` is microsecond-level; not in the same order of magnitude as network/route-matching cost on the request path.

The only visible micro-optimization: when `set_userinfo_header && userinfo_json.is_none() && store_claims_in_ctx` are simultaneously true, lines 60 and 71 of `userinfo.rs` would serialize twice — could reuse the same String. But the trigger condition is narrow; saves one microsecond-level alloc; the change benefit does not outweigh review/regression cost.

**Verdict**: inherent cost of the opt-in feature; not a performance issue. Any "OIDC/JWT hot-path claims JSON serialization" finding is closed per this entry, unless it is proved that the default is changed to `true` or claims size is ≥ tens of KB.

---

## Offline Controller Data Retention is a Business Requirement

**False-positive scenario**: in `ControllerRegistry` / `ResourceAggregator`, entries for offline Controllers are not periodically cleaned up; flagged as a "memory leak".

**Reality**: registration info and resource snapshots of offline Controllers are **business data that must be persistently retained** — the Admin API needs to display offline state; operators need to view the resource snapshot before going offline. Cleanup goes through an explicit operations path: `DELETE /api/v1/center/admin/controllers/{id}` cascades to delete four in-memory structures + DB row (see the Admin API table in `skills/09-misc/center-db.md`). Live resources (`stream_tx` / forwarding task / tonic HTTP/2 stream) are released immediately on `mark_offline`, not waiting for DELETE.

> Historical note: there used to be an `evict_stale(hours)` method + `offline_evict_hours` config, but it was never called by the scheduler — dead code. It was removed in 2026-04 fix #19; current semantics are "offline data has no TTL; cleaned only via Admin DELETE; but live resources are released the instant we go offline".

---

## `extract_status_json` Double Serialization is Cold Path + API Shape

**False-positive scenario**: `processor.rs:process_resource_inner` calls `extract_status_json(&parsed_obj)` once before and once after `update_status()`; each call internally `serde_json::to_value(obj)` fully serializes the entire object then `.get("status")`; flagged as "repeated serialization; CPU rises noticeably for HTTPRoute with many rules".

**Reality**:

- **Cold path**: `process_resource_inner` is only triggered by K8s resource watch events; not the request path. Watch event frequency is far below request frequency, and is queued asynchronously anyway.
- **The second serialization is unavoidable**: between the two calls, `self.handler.update_status(&mut parsed_obj, ...)` modifies status; the first `Value` cannot be reused. The "Approach A: serialize once and reuse the Value" in the report does not hold semantically.
- **What can really be saved is "serialize the entire object" → "serialize only the status subtree"**, but `extract_status_json<T: Serialize>` is generic and only requires `Serialize`; cannot directly access the `.status` field. To optimize, a `status_value() -> Option<Value>` method must be added to the K resource trait (affects trait definition + all impls); or each status type must implement `PartialEq` and `status_semantically_changed` must be rewritten (currently relies on JSON comparison to ignore non-semantic fields like `lastTransitionTime`; rewriting to struct-level comparison has even larger change surface).
- The comparison function `status_diff::status_semantically_changed(kind, old, new)` currently takes JSON strings; eliminating serialization cascades to its semantic comparison logic.

**Verdict**: cold path + API shape; the change surface (trait extension / comparison-semantics rewrite) far exceeds the benefit. Any "`extract_status_json` repeated serialization" finding is closed per this entry, unless an actual profile shows that controller CPU time is significantly spent in this function.

**Verdict**: not a memory leak; it is business-semantics retention; live resources are released immediately; what is retained is only static registration snapshots.

---

## Configuration Struct Field Ordering Causing Padding Waste

**False-positive scenario**: the audit flags configuration structs with `JsonSchema` such as `OpenidConnectConfig` (`src/types/resources/edgion_plugins/plugin_configs/openid_connect.rs:78-227`) where bool, String/u64/Option are interleaved; each instance wastes 30-50 B of alignment padding; suggests reordering by field size for tight packing.

**Reality**:

1. **Negligible scale**: plugin configs are instantiated per route; hundreds of instances × 30-50 B = a dozen KB total waste; not measurable in profile.
2. **Not on the hot path**: config is loaded once during reconcile; the request path only reads a few fields; cache-line utilization is unrelated to padding.
3. **Field order is part of the user-visible schema**: such structs typically have `#[derive(JsonSchema)]` + `serde(rename_all = "camelCase")`; field order maps directly to the generated CRD / JSON Schema documentation. Reordering by alignment would result in a counterintuitive "bool first / String later" order in the external schema; new fields would have to choose between "minimum padding" vs "logical grouping".
4. **Readability >> padding**: the current struct is sectioned by Required / Auth behavior / OIDC params / Token verification / Header forwarding / Network / Runtime / Security extras; maintainers can locate field categories at a glance. Breaking up grouping for an unobservable dozen-KB memory benefit is not worth it.

**Verdict**: not a memory issue. Any "config struct field-order optimization for padding / reorder by size" finding is closed per this entry, unless (a) the struct is allocated/copied ≥ a thousand times per second on the request path, or (b) per-instance padding ≥ KB scale.

---

## `HashMap::new()` Default Capacity on Reconcile / Full-Sync Paths

**False-positive scenario**: the audit flags Center `metadata_store::full_set` (`src/core/center/metadata_store/mod.rs:182-184`'s `new_cluster` / `new_service` / `new_gir`) and other `full_set` / reconcile paths' `HashMap::new()` for not using `with_capacity(data.len())`; suggests preallocation to reduce rehashing.

**Reality**:

1. **Cold path**: all `full_set` implementations live on Controller reload / Gateway config sync / Center fed_sync full push paths; trigger frequency = event level (Controller reconnect, reload, full re-push); **not request level**.
2. **Tiny scale**: in typical scenarios, `data.len()` is the count of a single Controller's PluginMetaData / single resource type, usually a few dozen entries (see the now-removed H6 review conclusion). Going from 0 to 64 capacity in HashMap involves ~3-4 rehashes; total cost O(64), microseconds level.
3. **Change surface vs benefit mismatch**: the project has 20+ `full_set` implementations and even more `HashMap::new()` on other reconcile / config-build paths. Bulk-changing to `with_capacity(data.len())` involves substantial signature/call-site changes; profile shows no benefit at all — same category as the already-judged "not an issue" "small-object operations on reconcile / config sync paths" (see the previous "Small-object clones on Resource reconcile / config sync paths are not issues").

**Verdict**: not a performance issue. Any "reconcile / full_set / config sync path `HashMap::new()` should use `with_capacity`" finding is closed per this entry, unless (a) the HashMap is allocated ≥ a thousand times per second on the request path, or (b) per-call `data.len()` is in the tens of thousands and rehash cost is observable.

---

## Access Log JSON Serialization Allocating a New String per Request is Not an Issue

**False-positive scenario**: `src/core/gateway/observe/access_log/entry.rs:103-110` `AccessLogEntry::to_json()` runs `serde_json::to_string(self)` on the request task context; allocates a new owned `String` per request; flagged as "hot-path per-request alloc; should use a buffer pool / defer to consumer thread / switch to `Bytes`/`Arc<str>`".

**Reality**:

1. **Architecture is a channel-based async logger**: call chain `routes/http/proxy_http/pg_logging.rs:42-63` → `AccessLogger::send(String)` → `LocalFileWriter` internal `SyncSender<String>::try_send` (`link_sys/providers/local_file/data_sender_impl.rs:30,184-187`, drops if full); a background `std::thread::spawn` consumes and writes to disk. **The channel boundary shape is owned `String`**; an owned conversion on the request task is unavoidable.

2. **Serialization cannot be moved to the consumer thread**: `AccessLogEntry<'a>` borrows from the request context throughout (`request_info`, `backend_context`, `stage_logs`, `ctx_map`, etc.; see entry.rs:26-62). Moving JSON-ification to the consumer thread is equivalent to first cloning all these fields over — more allocations, complicated lifetimes.

3. **Per-call cost is negligible**: a single entry is 1-5 KB; jemalloc size-class hit + a single String allocation; absolute cost is < microsecond level. On the same request path, serde traversal + plugin/route execution + upstream I/O are all at ms level; the alloc here is not measurable in profile.

4. **"Save alloc" approaches do not actually save**:
   - thread-local `Vec<u8>` + `serde_json::to_writer`: still need `String::from(buf)` to put into the channel; alloc count unchanged.
   - `Bytes` / `Arc<str>`: the `DataSender<String>` trait + `LocalFileWriter::SyncSender<String>` + every logger (access / mirror / debug / tcp / udp) all need changes; alloc not saved.
   - Defer to the consumer thread: must clone all borrowed fields; not worthwhile.

5. **Real bottleneck is downstream**: channel full means drop; disk I/O is the ceiling. Halving `to_json()` does not change throughput.

**Verdict**: not a performance issue. Same category as "OIDC/JWT claims JSON serialization is the opt-in feature itself" — once access log is enabled, **JSON serialization is the feature contract**; channel-boundary owned String is determined by API shape. Any "access log `to_json()` should use a buffer pool / switch to `Bytes` / defer to consumer thread" finding is closed per this entry, unless it is proved that (a) per-entry size is ≥ tens of KB at high QPS, or (b) profile measurement shows `to_json` alloc has significant share on the request path.

---

## CenterDb Single `Mutex<Connection>` is Not a Write-Blocking Issue

**False-positive scenario**: `src/core/center/db/mod.rs:28-30`'s `CenterDb { conn: Arc<Mutex<rusqlite::Connection>> }` flagged as "shared single connection across the Center; admin list query and registration write block each other"; suggests introducing an r2d2 connection pool or write-connection + multiple read-connections + WAL.

**Reality**:

1. **Trigger frequency is event level / operations level, not request level**:
   - `upsert_controller` → Controller register/reconnect (per-Controller startup once, or occasional on network jitter)
   - `mark_offline` → heartbeat timeout / disconnection (sparse events at Controller granularity)
   - `delete_controller` / `list_controllers` → Admin API (manual operations)
   
   The data-plane request path does not touch this lock at all. In the extreme case of 1000 Controllers reconnecting simultaneously, a single upsert is ~ a few dozen microseconds; full serialization ≪ 1s.

2. **SQLite itself is single-write**: in the default rollback journal mode, multi-connection writes still serialize on the file lock. Switching to an r2d2 pool only moves the wait from Mutex to inside SQLite; zero benefit on the write side. WAL mode allows read non-blocking on write, but read frequency in this scenario (admin LIST) is also extremely low; benefit is unobservable.

3. **Write path already isolated by spawn_blocking**: `fed_sync/server/mod.rs:171, 307` and `api/mod.rs:248, 295` all run inside `tokio::task::spawn_blocking`; the Tokio runtime threads are not blocked by Mutex; the blocking pool defaults to 512 threads, more than enough for sub-ms operations.

4. **The code itself is positioned as best-effort**: the comment at `server/mod.rs:161-164` makes it clear: "Persist registration to SQLite (best-effort, isolated from the hot path). Any failure here is logged and swallowed — we refuse to block fed-sync registration on DB availability" — DB writes have never been on the critical path; failures are swallowed.

5. **Limited data scale**: only one table `controllers`; row count = Controller count (typically < 1000); all queries are by-PK single-row upsert/delete or full-table select.

6. **Change surface does not justify benefit**: r2d2 + WAL requires new dependencies, checkpoint coordination, test rewrites; profile does not show this lock at all.

**Verdict**: not a performance issue. Any "CenterDb single Mutex blocks / should switch to connection pool or WAL" finding is closed per this entry, unless it is proved that (a) DB operations have entered the request path, or (b) profile shows this lock has significant wait time.

---

## Admin API `controller_summaries()` Multi-Call / Partial Field Discard is Not an Issue

**False-positive scenario**: the audit flags `ResourceAggregator::controller_summaries()` (`src/core/center/aggregator/mod.rs:123-135`) called in multiple Admin API handlers, with some call sites only taking one or two fields and discarding the rest; judged as "repeated full-field clone, CPU waste". Typical examples:

| Call site | Fields actually used | "Discarded" fields |
|-------|------------|-----------|
| `api/mod.rs:178` `list_controllers` | All 5 fields | None |
| `api/mod.rs:183` `list_clusters` | Only `.cluster` | controller_id / env / tag / online |
| `global_connection_ip_restriction_handlers.rs:53,161` `online_controllers` | filter `.online` + take `.controller_id` | cluster / env / tag |
| `region_route_handlers.rs:150,250,312` | Partial by controller_id / online | Same as above |

**Reality**:

1. **All are Admin API, not request path**: call sources are operations dashboard / CLI (list controllers / clusters / region routes / global IP restrictions); call frequency = human/operator scale, not data-plane request level.
2. **Limited scale**: per-Center, controller count typically a few dozen to a few hundred (far less than thousands). `ControllerSummary` only contains 2 short Strings + 2 normally-empty or very short `Vec<String>` + 1 bool; a single `collect` is at most a few KB.
3. **Multiple calls do not block each other**: `controller_summaries` internally is `RwLock::read()`; read locks are not exclusive; multi-handler concurrent calls have no lock contention.
4. **To "save" requires API-shape change**: to eliminate full-field clone, dedicated methods like `cluster_names()` / `online_controller_ids()` must be added to `ResourceAggregator` (API surface bloat + multiple repeated implementations), or change to `with_summaries(F)` callback (all call sites changed + read-lock-hold time controlled by callback, instead increasing concurrency risk). Profile shows no benefit at all.

**Verdict**: not a performance issue. Any "`controller_summaries` multi-call / partial field discard should use dedicated method or callback" finding is closed per this entry, unless it is proved that (a) the method is reshaped to be triggered per second on the request path, or (b) controller count is ≥ tens of thousands.

---

## `CenterWatchCacheRegistry::get_or_create` Slow-Path `to_string()` is Not an Issue

**False-positive scenario**: `src/core/center/watch_cache/registry.rs:38-39` does `controller_id.to_string()` twice in the write-lock branch (once as HashMap key, once passed to `CenterWatchCache::new` for ownership); flagged as "repeated stringify; should switch to `Arc<str>` / string interning".

**Reality**:

1. **Slow path, once per controller lifetime**: the fast path of `get_or_create` is `caches.read()` + `Arc::clone`, with no `to_string()`. The two `to_string()`s only execute on first insert in the write-lock branch, corresponding to controller registration or disconnect-reconnect — a typical cluster has a few to a few dozen controllers, so total occurrences are a few dozen.
2. **Not on the request path**: the call source is `fed_sync::server` after receiving a `RegisterRequest`; it is called once, not per-event / per-request.
3. **Negligible absolute cost**: 2 small String allocations per call (controller_id is typically a few dozen bytes); far below the protobuf deserialization and SQLite upsert cost of a single gRPC registration request.
4. **`Arc<str>` does not save much**: at most 1 allocation can be saved (the HashMap key still needs to independently own the key type, unless `HashMap<String, _>` is also changed to `HashMap<Arc<str>, _>`, which is a larger cascading change). An intern pool is even less applicable — this is not the "stringify the same string thousands of times per second" scenario.

**Verdict**: not a performance issue. Any "controller_id `to_string()` in `get_or_create` / registry registration path should switch to `Arc<str>` / interning" finding is closed per this entry, unless it is proved that this path is reshaped to per-request triggering or a single registration involves ≥ thousands of controllers.

---

## Route Handlers `record_cross_ns_refs` Loop `add_ref` is Not a Lock Hotspot

**False-positive scenario**: the audit flags `record_cross_ns_refs` (`resource_processor/handlers/{http,udp,grpc,tcp,tls}_route.rs:67-90` same pattern) for calling `manager.add_ref(...)` for every cross-namespace `backend_ref`, taking a write lock each time; suggests introducing `batch_update` for bulk-insert within a single lock.

**Reality**:

1. **Cold path — triggered by K8s resource watch event**: the call source is Route resource reconcile, event level (operator scale), not request level. The previous line in the same function `manager.clear_resource_refs(...)` itself takes a write lock once.
2. **Lock is `parking_lot::RwLock<RefMaps<V>>`** (`ref_manager.rs:170`), uncontended write ~25 ns; a single route typically has 1-10 backend_refs; pathological 100 → total lock overhead ~25 ns–2.5 µs. The path total is dominated by etcd RTT + resource parsing + status update; a few lock acquisitions are not measurable in totals.
3. **Change surface vs benefit**: to "save" you'd have to add `add_refs_batched` / `batch_update` methods to `BidirectionalRefManager`, or expose inner so the caller manages the lock — breaks encapsulation, expands API surface, requires changes in 5 handlers (http/udp/grpc/tcp/tls). Zero observable benefit on the cold path.
4. Same category as the existing "Small-object clones on Resource reconcile / config sync paths are not issues" — reconcile event level + small scale + cascading change surface.

**Verdict**: not a performance issue. Any "`record_cross_ns_refs` / similar reconcile handler with loop one-lock-per-item should be batched / done within a single lock" finding is closed per this entry, unless it is proved that (a) this path is reshaped to be triggered at request level, or (b) a single resource has ≥ thousand backend_refs and profile shows lock waits are significant.

---

## `BidirectionalRefManager` Lacking GC / Expiry Mechanism is Not a Memory Leak

**False-positive scenario**: the audit flags `BidirectionalRefManager` in `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_manager.rs` (forward `source_key → Set<V>` + reverse `value_key → Set<source_key>`) for not having `gc` / `cleanup` / `purge` / `expire` methods; "if delete events are lost or processing fails, refs will linger forever"; suggests adding a `gc(existing_values)` periodic task to validate. Typical examples: `SecretRefManager`, `ServiceRefManager`, `CrossNamespaceRefManager`, `BackendTLSPolicyTargetManager`, `stream_plugin_ref_mgr`.

**Reality**:

1. **It is not a cache; it is an index of the current K8s resource graph**: entry lifetime fully follows K8s resources; there is no independent TTL / expiry semantic. Treating it as a generic cache and applying "must have GC" is a category error.

2. **Each handler follows the clear-then-readd pattern**: every `process_*` path first calls `manager.clear_resource_refs(&resource_ref)` then loops `add_ref(...)`. Files covered: `handlers/{http,grpc,tls,tcp,udp}_route.rs:74` + `handlers/{http,grpc,tls,tcp,udp}_route.rs:~210/229` (cross_ns), `edgion_tls.rs:111/234`, `edgion_acme.rs:116/277`, `edgion_plugins.rs:82/135`, `backend_tls_policy.rs:130/178/300/306`, `gateway.rs:236/239/262/498/499/507`, `link_sys.rs:126/166`, `route_utils.rs:423/450`. Even if some event is missed in the middle, the next reconcile of that resource naturally clears stale refs.

3. **Each handler's delete path also calls `clear_resource_refs`**: paired with the process path (the second call site in the same files above).

4. **Watch missed events have a backstop**: Kubernetes informer has built-in periodic resync; watch reconnection replays missed delete events as DELETED; the handler runs clear again.

5. **Leader switchover / Controller restart fully resets**: `runtime.rs reset()` explicitly does `secret_ref_manager.clear()` + `service_ref_mgr.clear()` + `cross_ns_ref_mgr.clear()` + `processor_registry.clear_processors()`; init full-sync rebuilds the index from K8s. Any "long-term accumulated residue" goes to zero on restart.

6. **Memory is bounded**: total reference count = Σ(refs held by each K8s resource); directly bounded by resource graph size. `Set<V>` has built-in dedup (`add_ref` is idempotent); `remove_ref` / `clear_value_refs` also `HashMap::remove`s the bucket after the set is empty (`ref_manager.rs:216-218,222-224,268-271`) — no unbounded growth path.

7. **The suggested `gc(existing_values)` periodic task has side effects**: reconcile is two-phase clear-then-readd; when GC is concurrent with reconcile, it might delete a "currently exists but temporarily not in the index" value in the window between clear and add, instead introducing a bug. Even adding sync primitives is paying lock overhead and reconcile coordination complexity for a non-existent problem.

**Verdict**: not a memory issue. Any "`BidirectionalRefManager` / `SecretRefManager` / `ServiceRefManager` / `CrossNamespaceRefManager` should add GC / expiry / periodic check" finding is closed per this entry, unless it is proved that (a) some handler is missing the `clear_source_refs` / `clear_resource_refs` call on the delete path (a per-case bug; fix that location rather than add GC), or (b) under a long stable state, `stats().total_references` measurably grows monotonically and is significantly out of sync with the actual K8s resource count.

**Known fixed cases under (a)**:
- **c2a-007 (fixed)**: `SecretHandler::on_delete` did not purge the forward `secret_key → refs` entries in `SecretRefManager`. Fixed by adding `BidirectionalRefManager::clear_source_refs` and calling it in `on_delete` after the cascading requeue. `SecretRefManager` was also moved from per-processor `Arc` into `ControllerRuntime` (consistent with `service_ref_mgr` / `cross_ns_ref_mgr`) so that (i) `reset()` now correctly clears it, and (ii) its stats are visible in `/api/v1/debug/store-stats` for `cache_residue` leak detection.

---

## Workqueue Delay Heap Without Explicit Size Cap is Not an Issue

**False-positive scenario**: the `BinaryHeap<DelayedItem>` in `src/core/controller/conf_mgr/sync_runtime/workqueue.rs:330-384`'s `spawn_delay_loop` has no explicit capacity / size check; flagged as "unbounded growth can be abused / memory amplification risk"; suggests adding `max_heap_size` rejection or eviction policy.

**Reality**:

1. **Dedup by key before enqueuing**: `enqueue_after` (workqueue.rs:531-539) first calls `self.scheduled.insert(key)`; if already in the `scheduled` `DashSet`, returns false and does not send to `delay_tx`. The same key is in the heap at most once, until promote (workqueue.rs:357 `scheduled.remove`) re-permits scheduling.

2. **Each Workqueue serves only one resource type** (`workqueue.rs:250` "Each resource type should have its own workqueue instance"); heap size is naturally bounded:
   ```
   |heap| ≤ |unique keys scheduled-but-not-due under that Kind| ≤ total objects of that Kind in the cluster
   ```
   This upper bound is constrained by K8s apiserver/etcd themselves, typically thousands.

3. **delay_tx channel itself is bounded**: `mpsc::channel(config.capacity)` (default 5000, workqueue.rs:191, 295) — even if the heap could theoretically hold more, sender-side `delay_tx.send(...).await` will first block with backpressure.

4. **Pressure alarm exists**: workqueue.rs:541-551 logs `warn` when `pending + scheduled ≥ 80% capacity`; ops side is observable.

5. **Trigger chain has cycle/depth caps**: `max_trigger_cycles=5`, `max_trigger_depth=20` (workqueue.rs:193-194) block runaway cascade amplification; `promote_delayed_item` discards directly when limit is reached.

6. **The only scenario with abnormal heap growth is a resource type really having millions of objects in the cluster** — but in that case, informer/cache would have blown up first; adding a separate cap to the heap does not solve the root cause.

**Verdict**: not a memory/performance issue. Any "workqueue delay heap should add max_size / capacity check / LRU eviction" finding is closed per this entry, unless it is proved that (a) `scheduled` DashSet dedup is broken (per-case bug; fix the dedup rather than add a cap to the heap), or (b) profile measurement shows that under steady state heap size is significantly out of sync with the resource object count of that Kind.

---

## conf_sync Watcher Three-Layer mpsc(100) Chain is Not a Backlog Issue

**False-positive scenario**: the three `mpsc::channel(100)` in `conf_sync/cache_server/cache.rs:221,299` and `conf_sync/conf_server/grpc_server.rs:305` are counted as "per-watcher × 3 channels × 100 capacity × N events = 40000+ event backlog"; suggests reducing capacity or switching to a lock-free queue to alleviate memory.

**Reality**:

1. **The channel item is a batched `WatchResponse<T>`, not a single event**: at `cache.rs:140`, the watcher task uses `EventStore::get_events_from_sync_version` (`store.rs:187`) to bundle all unsent events from `from_version` into one `WatchResponse` and submit them. Channel capacity 100 = 100 *batches*; multiplying by event count is a unit error.

2. **All three mpsc layers submit via `.await`; backpressure is end-to-end**:
   - typed: `cache.rs:140` `sender.send(response).await`
   - simple: `cache.rs:321` `simple_tx.send(simple_response).await`
   - grpc outbound: `grpc_server.rs:331` `tx.send(Ok(response)).await`
   
   Downstream blocks → upstream blocks → watcher task stops pulling new events from EventStore. No `try_send`, no unbounded buffer, no event-loss bypass.

3. **EventStore is the source-of-truth and bounded**: `store.rs:153` is a circular queue (default capacity 200/kind; `mut_update` overwrites the oldest slot when full and advances `expire_sync_version`). When the watcher consumes slowly, events do not duplicate-pile-up in the channel — when the producer is stuck on `send().await`, new events overwrite the circular queue; next time the watcher wakes and finds `from_version < expire_sync_version`, triggering `WATCH_ERR_TOO_OLD_VERSION` (`store.rs:194`) so the client does a relist to realign. This is the designed fail-safe, not a memory leak.

4. **Steady state: channel close to empty**: `cache.rs:169` is `notify.notified()` driven; the watcher is woken once, pulls a batch, and goes back to wait. Only when a gRPC client peer's network is actually stuck or slow does the corresponding watcher's three-layer channel rise; this is itself a network failure symptom, not a memory issue determined by channel capacity.

5. **The three layers are decoupled with independent motivations, not stacked thoughtlessly**:
   - typed → simple: moves JSON serialization out of the store read lock (avoids long-held read locks)
   - simple → grpc: injects server reload detection (`grpc_server.rs:339` `check_interval` periodically checks server_id changes and proactively notifies the client to relist)
   
   Combining any two layers introduces new constraints (serializing inside the lock / mixing reload notifications into the watcher main loop). Capacity 100 at each layer is the empirical "leave a little buffer to absorb transient jitter" value; not a thoughtless large value being abused.

6. **Worst-case bound is controllable**: per-watcher all three layers full = 3 × 100 batches × ≤capacity events/batch × per-event a few KB ≈ tens of MB; requires that watcher's downstream connection to be completely stuck. N watchers all stuck simultaneously is a network/cluster-level failure scenario, not a channel capacity design defect; when it happens it is also auto-healed by `WATCH_ERR_TOO_OLD_VERSION` + relist.

**Verdict**: not a memory issue. Any "conf_sync watcher channel(100) chain causes backlog of N tens of thousands of events / should reduce capacity / should switch to lock-free structure" finding is closed per this entry, unless it is proved that (a) `try_send` or unbounded buffer bypasses backpressure, or (b) the EventStore circular queue's `WATCH_ERR_TOO_OLD_VERSION` relist path is broken, or (c) profile measurement shows that under normal network conditions the channel stays close to full at 100 for a long time.

---

## `status_diff::conditions_equal` O(n²) Linear Scan on Bounded Condition Arrays is Not an Issue

**False-positive scenario**: `conditions_equal` in `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_diff.rs` uses an O(n)-find inside an O(n) loop when matching new conditions against old conditions by `type`; flagged as "O(n²), should switch to `HashMap<&str, &Value>` for O(1) lookup". Same shape applies to the parent / ancestor / listener matchers in the same file (they all match by an identity field via `iter().find(...)`).

**Reality**:

1. **Cold path**: called from `processor.rs::status_has_changed` → `status_semantically_changed` → `conditions_equal`. Triggered only on Controller status reconcile (per resource update), never per request.

2. **n is hard-bounded and tiny**: per Gateway API / Edgion conventions, condition counts are ≤ 5 for Gateway-level status and ≤ 10 across all resource kinds. Worst case ~100 `Value` field comparisons of 5 short keys each, all in-memory, no allocations. Total cost is sub-microsecond.

3. **HashMap "optimization" can regress on small n**: the suggested `HashMap<&str, &Value>` fix incurs one heap allocation for the map plus a string hash per insert/lookup. For n ≤ 10 the constant overhead is comparable to or larger than the linear scan it replaces; the change trades easily-auditable code for a likely-neutral or slower path.

4. **The module's real concern is correctness, not throughput**: `status_diff` exists to ignore `lastTransitionTime` and similar non-semantic fields so we don't issue spurious K8s API writes. The linear-scan-by-type form makes it obvious to a reviewer that the matcher key is `type` and the compared fields are `status / reason / message / observedGeneration` (see `condition_fields_equal`). A HashMap-based version moves that logic two function calls away.

**Verdict**: not a performance issue. Code-clarity rewrite suggestions are not accepted unless (a) profile data shows status diff dominating Controller CPU, or (b) the condition count bound is broken by a new resource kind that produces dozens of conditions per status. Same reasoning applies to `parents_status_equal`, `ancestors_status_equal`, and the listener loop in `gateway_status_equal` — all match by identity (parentRef / ancestorRef / listener name) on the same bounded-cardinality input.
