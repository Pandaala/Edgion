# Fixed and Dynamic Metrics Interfaces

Date: 2026-10-14
Status: Independently approved design; implementation and final verification in progress.
Scope: Gateway, Controller, and shared Common metrics. The user authorized overall implementation after independent Astra plan approval.

## 1. Problem and requirements

Metric lifetime must follow label cardinality, not event frequency. Fixed process metrics have a finite label domain independent of resource creation. Dynamic metrics carry identities that can change indefinitely as resources are created, replaced, and deleted.

At the baseline reviewed before implementation, Gateway caches exporter handles in its process singleton and route matrices, while the shared exporter applies idle eviction to all metric kinds. Eviction removes the export registration but does not invalidate the application's retained handle. Further increments can update detached storage. A route matrix reports success based on slot presence, suppressing dynamic fallback even after exporter eviction.

Required outcomes:

1. Two distinct application interfaces: fixed metrics and dynamic metrics.
2. Fixed metrics cannot accept any CRD/resource/namespace/Service/LinkSys/module/endpoint/peer name, generation, timestamp, or other deployment-dependent identity label. Their initialized state remains exportable throughout the process lifetime. Dynamic cardinality exhaustion cannot prevent a valid fixed series from first being registered.
3. Dynamic metrics retain idle eviction and existing admission safeguards. An operation must not reuse an evicted exporter handle.
4. No vendor directory, dependency patch, exporter fork, or third-party source modification. No third-party source or version change is authorized. A direct declaration of the already-locked quanta 0.12.6 is approved solely to reuse the existing exporter RollingSummary public API.
5. No new metric families, request tracing, or resource-specific labels introduced to implement the mechanism.
6. Existing metric names, units, histogram enable switches/buckets, and intended aggregation remain unchanged unless an explicit migration decision below requires otherwise.

This is separate from the inventory's `self` / `business` category. Business traffic can produce fixed metrics (bytes and bounded token trends); infrastructure can produce dynamic metrics (LinkSys health).

## 2. Design baseline evidence and ownership

This section records the source evidence used to design the change. The selected implementation and its pending final verification are described below; these baseline observations are not claims about the migrated code.

| Owner / source | Baseline behavior | Consequence |
|---|---|---|
| `edgion-common/src/observe/metrics_api.rs` | Installs one recorder; `idle_timeout(MetricKindMask::ALL, ...)`; retains one render handle | Does not distinguish cardinality classes |
| `edgion-common/src/observe/cardinality_guard.rs` | Lifetime-distinct `Key` admission; default enforce; rejected handles are no-op | Same-key renewal is admitted; deleting a series does not refund a lifetime key |
| `edgion-gateway/src/observe/metrics/registry.rs` | Four eager handles and nine lazy `OnceLock<Counter>` slots in `GatewayMetrics` | All thirteen can remain cached after export eviction |
| `edgion-gateway/src/routes/http/proxy_http/pg_new_ctx.rs` and `pg_logging.rs` | Updates cached context/byte handles on requests | Repeated `global_metrics()` calls do not re-register metrics |
| `edgion-gateway/src/observe/metrics/route_handles.rs` | Stores precompiled Counter/Histogram arrays; `record()` tests indices, then returns true | Does not test export registration lifetime |
| HTTP and GRPC `conf_handler_impl.rs` | Compiles route matrices when configuration is built | New traffic alone does not rebuild these matrices |
| Gateway registry / scrape API | Existing weak-owner LB and Wasm keepalive mechanisms | Resource liveness can currently preserve an otherwise idle dynamic series |
| Gateway `cli/mod.rs` startup FIPS emissions | Writes fixed Gauges directly once or at startup; inventory constant definitions live elsewhere | Must migrate actual emit sites, not only the names registry |
| Common `system_metrics`, `fed_metrics`, and `conf_sync/cert_expiry_watch` | Mostly direct macros, bounded label conventions | Static string arguments do not enforce finite label domains |
| Controller `conf_mgr/sync_runtime/metrics.rs` | Cached process/per-kind handles plus event macros | Shares the interface concern; Controller currently configures idle timeout zero |
| Controller status/unknown-fields renderers | Hand-render resource snapshots and aggregate kinds | Outside exporter admission; existing snapshot bounds remain necessary |

Authoritative source inventory: `docs/en/ops-guide/observability/metrics-inventory.md`. Appendix A classifies every current inventory row. Classification does not assert that every existing string parameter is already type-safe.

Gateway defaults: duration disabled, idle timeout 600 seconds, recorder key cap 10,000, mode enforce. Controller recorder key cap also defaults to 10,000. Controller `metrics.resource_abnormal.hard_cap` separately defaults to 1,000 snapshot entries, outside recorder cap C; a snapshot entry can contribute more than one hand-rendered metric sample. The duration timeout is passed to the recorder even when duration collection is disabled. Controller currently installs timeout zero. Default backend request labels include Gateway identity, protocol, and status; route/backend identity are off. Gateway identity itself is dynamic, so this family is dynamic even under defaults.

## 3. Classification contract

### Fixed

A family is fixed only when every label has a closed, code-defined domain. Zero local labels are the simplest case. Bounded enum labels such as protocol, status group, reason, phase, direction, and resource **kind** are allowed when the accepted values are enumerated and enforced.

`&'static str`, a capped string length, a low current resource count, and a default-off label are not cardinality proofs. Do not convert leaked strings, user-defined plugin names, arbitrary kinds, or module identities into fixed labels.

Per-process maximum exporter keys:

`F = sum_over_fixed_families(product_of_allowed_label_domain_sizes)`.

Unlabelled families contribute one. Enabled features can select subsets but cannot enlarge domains at runtime. Histogram wire series have a separate bucket multiplier; this spec introduces no new fixed histogram exception.

Global `service` remains a fixed binary identity selected from the closed binary-role catalog, never an arbitrary configured instance/resource name. Prometheus target labels such as `instance` are outside this in-process bound.

### Dynamic

A family is dynamic if any supported label can derive from a resource/configuration identity or an open input domain. Route/backend label switches do not move a family between classes. An optional future optimization could collapse a fully identity-free configuration to a fixed descriptor, but that is outside this migration.

Request path, IP, user ID, trace ID, raw URL, query strings, secret material, and arbitrary error text remain prohibited, including through the dynamic interface. Dynamic does not mean arbitrary labels are authorized.

## 4. Canonical public API

Common owns the class/lifetime/admission mechanism; Gateway and Controller own semantic record functions and family definitions. Business callers continue calling those semantic functions. They cannot acquire raw exporter handles.

Illustrative interface, not final Rust signatures:

```rust
// Definitions are internal to the owning metric module.
// No constructor accepting arbitrary label strings is public.
struct FixedCounter { /* private backend handle */ }
struct FixedGauge { /* private backend handle */ }
struct DynamicCounter { /* immutable typed descriptor; no raw cached Counter */ }
struct DynamicGauge { /* immutable typed descriptor; no raw cached Gauge */ }
struct DynamicHistogram { /* immutable typed descriptor; no raw cached Histogram */ }

// Fixed enum labels are supplied when creating the fixed descriptor.
fixed.ctx_created.increment(1);
fixed.system_error(component_enum, reason_enum).set(1.0);

// Resource labels are compiled on the configuration path.
let requests = dynamic.backend_requests(compiled_route_labels);
requests.increment(1); // Resolve a current registration under the common contract.
```

Names, kinds, class, permitted label keys/domains, and supported operations belong to one canonical definition per family. A caller cannot pass `class: Fixed` next to arbitrary labels. Name/kind/class mismatch is a definition error detected before serving traffic. Fixed and dynamic backend partitions must not both export the same family.

Fixed wrappers may retain raw backend handles internally, because their backend never idle-evicts. Hot versus cold remains a secondary initialization choice: hot eager, cold first-event lazy. Scrapes do not initialize untouched cold metrics. Recorder installation precedes publication of any handle; initialization before install must not permanently cache no-op handles.

Dynamic wrappers may retain precompiled names, keys, label values, and bounded matrix indices. These descriptors are safe to cache. They must not expose raw `metrics::{Counter,Gauge,Histogram}` or allow `Arc<raw handle>` to escape. Raw backend access is restricted to the Common implementation and isolated backend tests.

A definition/inventory guard must enforce this boundary across all production crates; a scan limited to today's nine inventory source files is insufficient to prevent bypasses.

## 5. Lifetime and operation semantics

### Fixed state

Initialized means a series was registered into the intended installed runtime. Valid fixed definitions take admission priority; dynamic exhaustion cannot turn a first fixed event into a no-op. Once initialized, a fixed series retains its value and registration until process exit. Cold zero-event suppression stays intact. No resource-owner map or periodic synthetic event is needed. A fixed Gauge that remains at one must stay observable even when no new events occur (for example a latched system error).

### Dynamic state

State transitions:

`unregistered -> admitted/current -> idle/evicted -> admitted/new registration`.

A rejected new key stays rejected/no-op according to the existing policy; it cannot enter an unguarded backend through renewal. An existing admitted key can renew without consuming another lifetime key.

A successful operation linearizes registration resolution and mutation against eviction. Eviction concurrent with an emission may occur before that operation (operation renews) or after it (ordinary future idle expiry); it must not delete a fresh registration selected using stale scrape state or strand the emission in detached storage. Two concurrent renewals for the same typed key must update the same current registration. Same-key event families intentionally aggregate overlapping resource generations; authoritative state Gauges must preserve the actual live contributions rather than treating the newest CRD name as the only owner.

A Counter begins a new accumulation after true idle eviction. Histogram count/sum/buckets similarly begin a new registration epoch. This is an intentional reset, not preservation of deleted history. Prometheus historical samples and downstream retention are outside local eviction.

Dynamic Gauge operations require a semantic choice per family:

- `set(value)` describes the authoritative current value and is safe to restore into a new epoch.
- Paired `increment/decrement` or RAII ownership cannot blindly decrement a newly registered zero Gauge after the old epoch expires. Retain the real application ledger already owned by the subsystem and publish its current aggregate with `set`; do not maintain an unbounded duplicate metric ledger.
- Lock ordering is part of the correctness contract: capture subsystem snapshots before acquiring metric gates, or use an independently readable authoritative ledger with a documented acyclic order. Scrape cannot hold a metric gate and call into an owner lock when mutation holds that owner lock while acquiring the gate. Snapshot publication must not allow an older captured value to overwrite a newer mutation.
- Resource retirement must not create a fresh zero series merely to clear an already-expired Gauge. Recreated resources and old in-flight generations must not clear each other's live contributions.

Histogram renewal must not call `record(0)` or record synthetic samples. Histograms remain explicitly reviewed exceptions with the existing enable/bucket configuration.

Resource existence alone does not keep dynamic event metrics alive. A currently configured but unused route must be eligible for idle expiry. Conversely, explicit scrape-time sampling of an authoritative live resource state (LinkSys health) is real observation; its unchanged numeric value does not make the resource absent. Sampling stops on retirement, and its last series can expire. Document this distinction per state family.

## 6. Backend feasibility and required decision

### Fixed backend recommendation

Use an Edgion-owned class-aware facade around unchanged upstream components. A fixed Prometheus recorder partition has idle timeout disabled. A dynamic partition retains the configured timeout. One Common installation/exposure owner publishes both through the existing dedicated endpoint. This is one canonical mechanism with two lifetime policies, not two independent metrics subsystems.

Two partitions avoid fixed Counter/Gauge zero-touch lists and cannot accidentally pin a dynamic Histogram. They require one family routing table, consistent global labels and descriptions, and a deterministic merge with no duplicate HELP/TYPE or sample series. Hand-rendered Controller blocks retain their existing ownership and are appended exactly once.

### Dynamic backend is an implementation gate

The installed `metrics-exporter-prometheus` 0.18.3 has public registration, render, and upkeep APIs, but no public per-key registration-membership/generation token or sample-free Histogram touch. Its idle eviction happens during render snapshot collection; `run_upkeep` drains Histograms and descriptions rather than providing a separately controllable eviction pass.

Merely running `register_counter(key).increment(n)` on every event fixes sequential stale caching but does **not** establish the concurrent lifetime contract. A scrape can snapshot the old generation and delete by key after registration resolution. A clock check, fixed renewal interval, or application scrape epoch without serialization is also insufficient.

An operation/render coordination gate can establish correctness with unchanged dependencies, but holding a write gate around the whole dynamic `render()` blocks request updates for potentially the entire serialization. That is a reference correctness model, **not an approved production design**. Direct per-event re-registration also replaces the current route matrix's zero-allocation/zero-lookup/zero-lock fast path. The current dependency converts `Generational` state into a fresh `Arc`-backed metrics handle on registration, including registry hits; repeated acquisition has a heap-allocation cost even with precompiled labels.

Before production implementation, choose and independently review a backend that proves all of:

1. Eviction/renewal linearization and conditional Gauge retirement without resurrecting expired keys, using only actual public APIs or an explicitly bounded Edgion-owned metric store. A register/update gate alone does not provide the missing current-registration query needed by retirement.
2. No retained raw dynamic handles outside a valid operation scope.
3. No process-wide request lock waiting for full scrape serialization.
4. Precompiled labels on the request path; measured cost of registry resolution and any concurrency coordination.
5. A memory bound including current export state, lifetime admission keys, descriptors, in-flight retired configuration, and any new renewal/pending state.

An Edgion-owned bounded store is a fallback to evaluate, not authorization to write another exporter. It must reuse existing cache/time/histogram components where suitable and supply a reviewed complexity/bucket/heap ledger. Sharded exporter partitions likewise require proof of merge semantics, per-shard work bounds, and measured tail latency; sharding alone does not remove the register/evict race.

If none meets these gates without a dependency patch or an unacceptable hot-path regression, record the dynamic implementation as unresolved. Do not substitute resource-lifetime pinning or advertise a complete ARCH-043 fix. The class split and interface contract remain useful independently.

## 7. Cardinality and memory safeguards

Preserve the existing lifetime-distinct admission model and defaults. Expiry/deletion does not refund keys: refunding allows timestamp-named resources to cycle indefinitely around the safety net. Same-key renewal does not increase the ledger. Do not silently redefine the cap as a live-series cap.

### One total limit, with simple fixed-class priority

The user requested a simpler implementation on 2026-10-14. Keep the single existing `metrics.cardinality_cap` setting and the current shared lifetime-distinct key count. Do not introduce a fixed reservation, a separate dynamic budget, startup domain-capacity computation, C < F startup failures, per-family priorities, or a strict concurrent limit redesign.

In positive-cap enforce mode the admission rule is:

```text
if the key was already admitted: allow renewal
else if the canonical definition and label tuple are valid fixed class: allow and count
else if the shared distinct-key count is below the configured cap: allow and count
else: reject the new dynamic key
```

Only the canonical catalog can establish fixed class; callers cannot supply arbitrary resource labels with a fixed flag. All admitted keys, fixed and dynamic, contribute to the same count. The two existing guard self-metrics remain fixed and available. Fixed cold keys are admitted only on first real use; scrapes do not initialize untouched cold metrics.

This is a total **soft admission threshold**, with a deliberate fixed-class exception: if dynamic traffic has filled the threshold, a later first fixed event is still collected. The total can therefore exceed the configured value by those later fixed keys, plus the existing concurrent dynamic-admission overshoot. The fixed exception remains finite because fixed labels have a closed domain; it cannot follow resource churn. Do not advertise an exact 10,000-key ceiling. A strict total maximum together with guaranteed late fixed admission would require reservation or displacement, which the user declined in favor of simplicity.

Preserve the current approximate concurrent admission mechanism; no admission mutex is added to each operation. Same-key renewal does not consume another key, and expiry/deletion never refunds lifetime keys. Dynamic overflow drops metrics only, never business traffic.

Cap zero and observe mode retain their existing disabled/monitoring semantics; they do not promise a maximum. Neither class priority nor this limit resolves the separate dynamic exporter eviction race.

The existing cap is deliberately soft under concurrent new registrations; no strict bound is claimed for current code. Cap zero disables guarding; observe mode retains lifetime keys beyond the cap. Neither setting provides a memory-bound guarantee. Key length and owned label bytes require explicit limits as well as key-count bounds. At the baseline before implementation, the upstream exporter buffered raw Histogram observations between drains; a key cap did not bound samples retained by one hot Histogram. A fixed drain interval alone is not a mathematical heap bound without limits on input rate/concurrency and pending samples, and `build_recorder()` does not automatically run upkeep. The selected dynamic adapter instead uses bounded atomic classic buckets or the unchanged bounded rolling Summary, independent of scrape cadence, and limits prepared key bytes.

Current default enforce mode has admission overshoot conditional on the number of concurrent first registrars (no numeric hard overshoot bound is presently proven). The proposed simple rule additionally permits late fixed keys above the threshold; no exact hard bound is claimed. Histograms multiply emitted wire series; a 10,000 key cap does not imply 10,000 Prometheus samples. For B explicit classic buckets the family emits B+3 series per key (finite buckets, +Inf, sum, count).

For 10 new timestamp-named routes per minute, assume enabled route labels and at most M distinct registrations per route: lifetime keys grow approximately `10 * M` per minute until enforcement. Baseline route matrices pre-registered protocol/status combinations before traffic; the selected matrices prepare cold descriptors and admit keys only on real events. A 600-second timeout does not guarantee an exact `100 * M` live plateau: eviction depends on scrape observation and concurrent activity. Verify measured behavior rather than treating timeout as a hard TTL.

At the baseline, expired raw exporter data could remain allocated through retained handles. The selected renewable dynamic proxies retain lifetime identity metadata, not accumulation epochs; idle accumulation is released independently of proxy ownership. Bounding export output alone is insufficient. Compiled descriptors and in-flight configurations are a separate configuration-lifetime bound, never a global forever-growing descriptor map.

Any new in-memory registry needs bounded diagnostics, `StoreStatsResponse` coverage, cache-residue checks, and empty-store tests. Never emit CRD identity or generation as a new diagnostic metric label.

## 8. Migration boundaries

1. Common: canonical definitions, fixed/dynamic facade, admission, install/render ownership, isolated recorder test hooks. Preserve one installation authority.
2. Gateway singleton: migrate the original thirteen fixed slots. Extend fixed treatment to all proven closed-domain families, not merely those thirteen.
3. Route matrices: retain compiled indices/labels but replace raw Counter/Histogram cells with dynamic descriptors. Keep sparse backend alignment, fallback label parity, integration-test behavior, and all-before-write validation that prevents double counting. Descriptor construction does not register/admit dynamic keys; the first real operation does. This intentionally removes config-time zero-valued protocol/status cells and avoids exhausting lifetime admission solely through unused route churn. Update tests/docs that relied on eager zero cells. Current-event rejection remains silent in the metric path and does not reject traffic.
4. Gateway dynamic free functions: circuit breaker Service identity, LinkSys identity, Wasm module identity, plugin policy open names, and route families use the dynamic interface.
5. LB/Wasm ownership: do not generalize existing weak-owner keepalive. Migrate these owners into descriptors/current-state publication; remove superseded keepalive only with targeted counter/RAII/shared-gauge regression evidence. Split the fixed `wasm_modules_loaded` Gauge from the dynamic released Counter currently held together. Circuit-breaker ejected-endpoint increment/decrement accounting also needs authoritative current-state migration; it cannot be fixed by handle renewal alone. A Drop path must not block on a scrape-wide lock or resurrect retired metric identities.
6. Controller/Common: migrate per-process, closed kind/reason/job/role/event families to fixed wrappers. Replace open string conventions with closed vocabulary validation/types. The identity-bearing hand-rendered resource snapshots remain dynamic bounded snapshots; they do not require exporter handles. `edgion_controller_unknown_config_fields` has only a closed four-kind domain and migrates to a fixed sampled Gauge: preserve the diagnostics scan owner, initialize a kind only when first nonzero, set zero on recovery, and thereafter continue exporting zero. Remove its old hand-rendered metric block to prevent duplicates. This deliberately changes its current absence-on-recovery output; diagnostic API behavior is unchanged.
7. Documentation/guards: update coding rules from hot/cold-first to fixed/dynamic-first, correct timeout scope/default text, classify inventory rows, and prevent new raw API bypasses. Existing route performance knowledge must be re-evaluated explicitly; its current fast-path assertions cannot be silently discarded.

Fixed memory and dynamic safety are cross-cutting changes. Migration batches must carry an exact file allow-list and independent design/result review; no repository-wide unrelated cleanup. The original ARCH-043 remains open until the accepted scope and dynamic regression tests are satisfied.

## 9. Acceptance and verification

- Definition tests reject fixed CRD names, arbitrary plugin names, arbitrary kinds/reasons, undeclared labels, and family kind/class mismatches. Static lifetimes alone do not pass these tests.
- Fixed tests: initialized counters and persistent nonzero Gauges survive multiple idle periods; cold untouched families remain absent; first-event initialization concurrent with scrape preserves the event; installation order cannot freeze no-op handles.
- Dynamic tests: idle expiry, same-key renewal, concurrent renewal, and forced interleavings between scrape snapshot/deletion and operation; no detached updates or duplicate counting; counter reset is explicit.
- Histogram tests: observed count/sum/buckets match actual samples only, including renewal; disabled request duration remains absent; bucket multiplication stays unchanged.
- Gauge tests: fixed sampled-kind nonzero-to-zero recovery and untouched-kind absence, live contribution restoration, old-generation Drop after renewal, overlapping resource replacement, last-owner retirement, and no negative count caused by an expired epoch.
- Admission tests: one shared count including both classes, rejection of new dynamic keys at threshold, late first fixed event above threshold, arbitrary fixed labels rejected by the catalog, untouched cold absence, descriptor creation without consuming admission, same-key renewal without double counting, existing concurrent soft-cap semantics, self-metrics survive idle, no key refund on deletion, documented observe/zero behavior.
- Dynamic route integration: create/request/delete timestamp-named routes repeatedly; check retired output/state, admission growth and enforcement, descriptor/owner residue, and stable unrelated fixed series. Include 2xx-active/5xx-idle behavior.
- Benchmark current and proposed route request writes, fixed writes, rejected writes, rare-event initialization, and concurrent near-cap scrapes. Record throughput, allocations, contention, and p95/p99 latency. Acceptance thresholds must be set in the implementation plan before comparing measurements; do not approve an unmeasured request-path regression.
- Follow repository unit-test layout and reuse the existing shared integration topology. Existing integration tests remain intact; add meaningful regressions instead of replacing them.
- Implementation checks: targeted unit/integration tests, format, workspace compile/lint, docs validation, metrics inventory, unit layout, dependency/Pingora source policy, and cache-residue checks when stores change. This task includes implementation and verification; it does not claim a full test-suite run. Full `cargo test --all` requires separate explicit authorization.

## 10. Source and knowledge references

- `skills/00-design-rules.md`
- `skills/03-coding/observability/01-metrics.md`
- `skills/04-review/cpu-memory/regression-guards/route-unit-metric-handles.md`
- `skills/04-review/observability/intentional-design/metrics-registry-cold-oncelock-pattern-not-extracted.md`
- `skills/04-review/concurrency/intentional-design/lock-around-atomic-metrics.md`
- `skills/04-review/link-sys/intentional-design/health-metric-free-function-and-cardinality.md`
- Historical source input: `tasks/review-architecture-20261009/ARCH-043-cached-metric-handle-lifetime.md` (closure remains subject to final verification).

The old cold-slot extraction decision concerned boilerplate, not this lifetime/class contract. Baseline route performance knowledge described the cached-handle optimization without proving that it survived exporter eviction; the scoped knowledge migration now describes renewable descriptors. The baseline inventory's zero/no-op idle-timeout default disagreed with Gateway's 600-second timeout applied to all kinds. The inventory now documents the selected policy: Gateway dynamic epochs default to 600 seconds, while fixed metrics do not expire.

## Appendix A. Current family classification

`Fixed` means eligible after finite-domain validation/type enforcement. `Dynamic snapshot` means a hand-rendered bounded resource snapshot, outside exporter lifetime handling. The table covers the current catalog, not new families. Existing plugin policy names are conservatively dynamic because their API accepts operator-derived strings; converting them to a fixed vocabulary requires a separate proof and unknown-type decision.

| Family | Kind | Process | Class | Current local labels |
|---|---|---|---|---|
| edgion_ctx_created_total | Counter | gateway | Fixed | none |
| edgion_ctx_active | Gauge | gateway | Fixed | none |
| edgion_access_log_dropped_total | Counter | gateway | Fixed | none |
| edgion_ssl_log_dropped_total | Counter | gateway | Fixed | none |
| edgion_tcp_log_dropped_total | Counter | gateway | Fixed | none |
| edgion_tls_log_dropped_total | Counter | gateway | Fixed | none |
| edgion_udp_log_dropped_total | Counter | gateway | Fixed | none |
| edgion_access_log_backfill_ingested_total | Counter | gateway | Fixed | none |
| edgion_access_log_backfill_failed_total | Counter | gateway | Fixed | none |
| edgion_config_reload_signals_total | Counter | gateway | Fixed | none |
| edgion_config_relist_total | Counter | gateway | Fixed | none |
| edgion_config_event_parse_errors_total | Counter | gateway | Fixed | kind |
| edgion_gateway_waf_compile_total | Counter | gateway | Fixed | result (ok/error) |
| edgion_gateway_waf_compile_microseconds_total | Counter | gateway | Fixed | result (ok/error) |
| edgion_gateway_waf_generation_bytes | Gauge | gateway | Fixed | none |
| edgion_gateway_waf_runtime_bytes_active | Gauge | gateway | Fixed | none |
| edgion_gateway_waf_body_bytes_inspected_total | Counter | gateway | Fixed | direction (request/response) |
| edgion_gateway_waf_execution_total | Counter | gateway | Fixed | phase (request_headers/request_body/response_headers/response_body/logging) |
| edgion_gateway_waf_execution_microseconds_total | Counter | gateway | Fixed | phase (request_headers/request_body/response_headers/response_body/logging) |
| edgion_request_body_storage_io_failures_total | Counter | gateway | Fixed | operation (spill_create/spill_write/replay_read/materialize_read/snapshot_read), reason (admission_timeout/admission_closed/execution_timeout/join_error/io_error) |
| edgion_request_body_storage_io_inflight | Gauge | gateway | Fixed | none |
| edgion_link_sys_es_bulk_channel_full_total | Counter | gateway | Fixed | none |
| edgion_link_sys_health | Gauge | gateway | Dynamic | link |
| edgion_link_sys_health_flips_total | Counter | gateway | Dynamic | link, direction (recovered/degraded) |
| edgion_ldap_auth_errors_total | Counter | gateway | Fixed | kind (unreachable/timeout/bind_fail/tls_san_mismatch/service_bind_fail/search_fail/starttls_rejected) |
| edgion_gateway_outbound_tls_san_mismatch_total | Counter | gateway | Fixed | plugin (ldap_auth/openid_connect) |
| edgion_webhook_resolve_errors_total | Counter | gateway | Fixed | reason (not_found/unhealthy/rate_limited/status_error/call_failed/authority_rejected) |
| edgion_outbound_egress_blocked_total | Counter | gateway | Fixed | reason (loopback/link_local/blocked_cidr/non_dialable/private/unspecified) |
| edgion_gateway_upstream_authority_rejected_total | Counter | gateway | Fixed | reason (render_empty/invalid_label) |
| edgion_outbound_success_predicate_fail_total | Counter | gateway | Fixed | none |
| edgion_outbound_template_fallback_total | Counter | gateway | Fixed | field (timeout_ms) |
| edgion_config_conflict_total | Counter | gateway | Fixed | field (constant set: outbound_ca, outbound_client_identity) |
| edgion_outbound_injected_header_skipped_total | Counter | gateway | Fixed | none |
| edgion_gateway_fips_module_enabled | Gauge | gateway | Fixed | none |
| edgion_gateway_fips_host_enabled | Gauge | gateway | Fixed | none |
| edgion_gateway_fips_host_detect_error | Gauge | gateway | Fixed | none |
| edgion_gateway_fips_policy_active | Gauge | gateway | Fixed | module_enabled (true/false) |
| edgion_wasm_module_load_total | Counter | gateway | Dynamic | module, result (ok/error) |
| edgion_wasm_module_compile_duration_ms | Histogram | gateway | Dynamic | module |
| edgion_wasm_module_released_total | Counter | gateway | Dynamic | module |
| edgion_wasm_instance_instantiate_total | Counter | gateway | Dynamic | module |
| edgion_wasm_instance_discard_total | Counter | gateway | Dynamic | module, reason (poisoned/over_capacity) |
| edgion_wasm_pool_size | Gauge | gateway | Dynamic | module |
| edgion_wasm_hook_duration_ms | Histogram | gateway | Dynamic | module, phase (request_headers/request_body/response_headers/response_body/http_call_response/tick) |
| edgion_wasm_trap_total | Counter | gateway | Dynamic | module, phase |
| edgion_wasm_fail_mode_total | Counter | gateway | Dynamic | module, reason, mode (open/close) |
| edgion_wasm_callout_total | Counter | gateway | Dynamic | module, result (ok/error/denied/over_limit) |
| edgion_wasm_modules_loaded | Gauge | gateway | Fixed | none |
| edgion_wasm_shared_data_bytes | Gauge | gateway | Dynamic | module |
| edgion_wasm_body_pause_unsupported_total | Counter | gateway | Dynamic | module |
| edgion_gateway_plugin_policy_blocked_total | Counter | gateway | Dynamic | plugin |
| edgion_gateway_plugin_policy_bypassed_total | Counter | gateway | Dynamic | plugin |
| edgion_geoip_db_loaded | Gauge | gateway | Fixed | none |
| edgion_geoip_db_build_epoch_seconds | Gauge | gateway | Fixed | none |
| edgion_geoip_update_failed_total | Counter | gateway | Fixed | reason (fetch/encoding/size_cap/stall/validation/rename/disk/deadline) |
| edgion_geoip_lookup_panic_total | Counter | gateway | Fixed | none |
| edgion_gateway_retry_budget_denied_total | Counter | gateway | Fixed | none |
| edgion_conf_sync_cert_expiry_timestamp_seconds | Gauge | both | Fixed | side (server/client) |
| edgion_system_error_state | Gauge | both | Fixed | component, reason |
| edgion_fed_connections_active | Gauge | controller | Fixed | role (center/controller) |
| edgion_fed_connection_events_total | Counter | controller | Fixed | role, event (connected/disconnected/reload) |
| edgion_fed_connection_duration_seconds_last | Gauge | controller | Fixed | role |
| edgion_fed_watch_events_total | Counter | controller | Fixed | kind, direction (sent/recv) |
| edgion_fed_ready_gate_wait_seconds_last | Gauge | controller | Fixed | none |
| edgion_fed_rbac_denied_total | Counter | controller | Fixed | verb, kind, source (center/cli_token/unknown) |
| edgion_fed_kill_switch_state | Gauge | controller | Fixed | none |
| edgion_fed_client_running | Gauge | controller | Fixed | none |
| edgion_controller_resource_abnormal | Gauge | controller | Dynamic snapshot | kind, namespace, name, err, warn |
| edgion_controller_unknown_config_fields | Gauge | controller | Fixed | kind |
| edgion_controller_init_sync_duration_seconds_last | Gauge | controller | Fixed | kind |
| edgion_controller_active | Gauge | controller | Fixed | none |
| edgion_controller_resources_watched | Gauge | controller | Fixed | kind |
| edgion_controller_leader | Gauge | controller | Fixed | none |
| edgion_controller_leader_transitions_total | Counter | controller | Fixed | none |
| edgion_controller_reload_client_notifications_total | Counter | controller | Fixed | none |
| edgion_controller_status_writes_total | Counter | controller | Fixed | kind, backend (kubernetes/filesystem/etcd), result (success/error) |
| edgion_controller_status_writes_skipped_total | Counter | controller | Fixed | kind |
| edgion_controller_status_conditions_max | Gauge | controller | Dynamic snapshot | kind, namespace, name |
| edgion_controller_unexpected_apply_during_init_total | Counter | controller | Fixed | kind |
| edgion_controller_leader_job_alive | Gauge | controller | Fixed | leader_job (acme/conf_sync_rotation/credential_manager) |
| edgion_metrics_series_rejected_total | Counter | both | Fixed | none |
| edgion_metrics_cardinality_limit_active | Gauge | both | Fixed | none |
| edgion_backend_requests_total | Counter | gateway | Dynamic | configurable (see backend_requests.labels) |
| edgion_gateway_request_duration_ms | Histogram | gateway | Dynamic | configurable (see request_duration.labels); disabled by default |
| edgion_gateway_request_bytes_total | Counter | gateway | Fixed | none |
| edgion_gateway_response_bytes_total | Counter | gateway | Fixed | none |
| edgion_gateway_genai_tokens_total | Counter | gateway | Fixed | direction (input/output), provider (openai/openai_compatible/anthropic), usage_source (reported/estimated) |
| edgion_mirror_requests_total | Counter | gateway | Fixed | status (ok/body_abandoned/channel_full/buffer_limit/connect_err/connect_timeout/write_err/timeout) |
| edgion_conn_ip_restriction_events_total | Counter | gateway | Fixed | result (overlay_missing/overlay_disabled/overlay_type_mismatch/overlay_invalid/fail_closed_profile_missing/denied) |
| edgion_geoip_denied_total | Counter | gateway | Fixed | layer (stage1/tcp), reason (deny_country/allow_miss/db_unavailable/lookup_failed/ref_unavailable) |
| edgion_connection_rate_limit_rejected_total | Counter | gateway | Fixed | dimension (listener/source_ip), stage (local/redis) |
| edgion_connection_rate_limit_redis_failure_total | Counter | gateway | Fixed | dimension (listener/source_ip), cause (unavailable/runtime) |
| edgion_circuit_breaker_ejections_total | Counter | gateway | Dynamic | service, class (gateway/application/local_origin) |
| edgion_circuit_breaker_ejected_endpoints | Gauge | gateway | Dynamic | service |
| edgion_circuit_breaker_rejections_total | Counter | gateway | Dynamic | service |
| edgion_gateway_circuit_breaker_shed_total | Counter | gateway | Dynamic | service |
| edgion_lb_degrade_selections_total | Counter | gateway | Dynamic | service |
| edgion_rr_selector_truncated_total | Counter | gateway | Fixed | none |
| edgion_region_route_config_events_total | Counter | gateway | result (setting_reference_unavailable/setting_missing/setting_disabled/setting_type_mismatch/setting_invalid/hash_modulo_invalid) |
| edgion_acme_challenge_lookup_total | Counter | gateway | Fixed | result (hit/expired/domain_mismatch/not_found/empty_host) |

Catalog coverage: 102 distinct rows. Global binary `service` is omitted from the local-label column.

## Selected implementation and verification boundary

Common owns one validated class catalog and class-specific facade. Valid fixed
handles use the unchanged exporter without idle expiration. Dynamic descriptors
may bind an opaque recorder proxy during preparation, without admitting an export
identity or allocating an accumulation epoch. First actual recording resolves a
stable slot in the existing lifetime ledger. Optional epoch aggregation expires
under a per-slot gate and the same descriptor renews it on later traffic. Expiry
never refunds quota. No third-party source, vendor directory, or fork is used.

Counter updates and fresh Counter snapshots use shared slot gates; classic
Histogram snapshots use exclusive gates to capture coherent bins/count/sum.
Classic aggregation uses narrow atomic first-match bins matching upstream
record_many, including arbitrary configured order, duplicate and nonfinite bounds.
Summary aggregation reuses public Distribution/RollingSummary unchanged, with
three 20-second windows and cumulative count/sum. A scrape-only mutex bounds
snapshot retention through serialization. Formatting and retired dense sketch
destruction occur after operation gates release. No request waits for full render.

GaugeContribution tokens retain each owner's committed scalar even before first
successful admission. First attachment adds the entire current scalar once;
committed deltas are published in subsystem mutation order. Overlapping Wasm
SharedData, module pools and circuit-breaker owners aggregate independently.
Conditional retirement cannot recreate an expired gauge. Owner scopes and attached
contributions are separate diagnostics, and both must release after resource wipe.

Explicit try_new preparation reports MetricDefinitionError. Convenience event
macros fail closed without business failure, admission, or new per-request logging.
Canonical catalog and inventory guards enforce family/kind/class/label consistency;
fixed families cannot contain resource identities. Two bounded Controller snapshot
families retain their separate limits. Unknown-fields per-kind sampling uses fixed
gauges, cold absent before a first finding and zero after recovery.

Current component benchmarks exercise the production facade, not system QPS:
zero warm fixed/dynamic/rejected allocations; eight-writer Counter and paired
classic Histogram latency under actual 10,000-key scrapes; dense three-window
positive/negative Summary production snapshot capture. Final receipts and targeted
unit/integration/workspace checks are recorded under
impl/architecture-ARCH-043/overall-verification. Preliminary or historical checks
are not final acceptance. Full cargo test --all is not claimed.
