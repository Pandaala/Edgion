use super::*;
use crate::plugins::http::wasm::host_abi::WasmPhase;
use edgion_resources::plugin_configs::WasmRawConfig;
use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;
use std::collections::HashMap;

fn write_temp_wasm(wat: &str) -> tempfile::NamedTempFile {
    // wat text compiles via `Module::new` directly, but the file loader
    // reads raw bytes off disk, so serialize real wasm bytes via the
    // `wat` crate first.
    let bytes = wat::parse_str(wat).expect("wat parses");
    let f = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(f.path(), &bytes).unwrap();
    f
}

const MINI_GUEST: &str = r#"
    (module
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 1024))
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        global.get $heap)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// Guest whose request-headers hook hits `unreachable` (simulated panic).
const TRAP_GUEST: &str = r#"
    (module
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) unreachable)
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

const HOST_PANIC_SENTINEL: &str = "host-panic-sensitive-sentinel";

#[derive(Clone, Copy, Debug)]
enum PanicSite {
    Instantiate,
    Initialize,
    Start,
    RootContext,
    VmStart,
    Configure,
    RequestContext,
    RequestHeaders,
    ResponseHeaders,
    RequestBody,
    ResponseBody,
    HttpCallResponse,
    Tick,
    Done,
    Delete,
}

fn panic_guest(site: PanicSite) -> String {
    let call = "(call $host_panic)";
    let module_start = matches!(site, PanicSite::Instantiate).then_some(call).unwrap_or("");
    let initializer = match site {
        PanicSite::Initialize => format!(r#"(func (export "_initialize") {call})"#),
        PanicSite::Start => format!(r#"(func (export "_start") {call})"#),
        _ => String::new(),
    };
    let root_context = matches!(site, PanicSite::RootContext | PanicSite::RequestContext)
        .then_some(call)
        .unwrap_or("");
    let vm_start = matches!(site, PanicSite::VmStart).then_some(call).unwrap_or("");
    let configure = matches!(site, PanicSite::Configure).then_some(call).unwrap_or("");
    let request_headers = matches!(site, PanicSite::RequestHeaders).then_some(call).unwrap_or("");
    let response_headers = matches!(site, PanicSite::ResponseHeaders).then_some(call).unwrap_or("");
    let request_body = matches!(site, PanicSite::RequestBody).then_some(call).unwrap_or("");
    let response_body = matches!(site, PanicSite::ResponseBody).then_some(call).unwrap_or("");
    let http_call_response = matches!(site, PanicSite::HttpCallResponse)
        .then_some(call)
        .unwrap_or("");
    let tick = matches!(site, PanicSite::Tick).then_some(call).unwrap_or("");
    let done = matches!(site, PanicSite::Done).then_some(call).unwrap_or("");
    let delete = matches!(site, PanicSite::Delete).then_some(call).unwrap_or("");
    format!(
        r#"
            (module
              (import "env" "proxy_log"
                (func $panic_import (param i32 i32 i32) (result i32)))
              (memory (export "memory") 1)
              (func $host_panic
                (drop (call $panic_import (i32.const 0) (i32.const 0) (i32.const 0))))
              (func $module_start {module_start})
              (start $module_start)
              {initializer}
              (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
              (func (export "proxy_on_context_create") (param i32 i32) {root_context})
              (func (export "proxy_on_vm_start") (param i32 i32) (result i32)
                {vm_start}
                (i32.const 1))
              (func (export "proxy_on_configure") (param i32 i32) (result i32)
                {configure}
                (i32.const 1))
              (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
                {request_headers}
                (i32.const 0))
              (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
                {response_headers}
                (i32.const 0))
              (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32)
                {request_body}
                (i32.const 0))
              (func (export "proxy_on_response_body") (param i32 i32 i32) (result i32)
                {response_body}
                (i32.const 0))
              (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32)
                {http_call_response})
              (func (export "proxy_on_tick") (param i32) {tick})
              (func (export "proxy_on_done") (param i32) (result i32)
                {done}
                (i32.const 1))
              (func (export "proxy_on_delete") (param i32) {delete})
              (func (export "proxy_abi_version_0_2_0"))
            )
            "#
    )
}

fn panicking_linker(
    should_panic: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    failed_tick_period: Option<u64>,
) -> impl FnOnce(&mut Linker<HostState>) -> Result<(), String> + Send {
    move |linker| {
        linker
            .func_wrap(
                edgion_wasm::host::MODULE,
                edgion_wasm::host::PROXY_LOG,
                move |mut caller: wasmtime::Caller<'_, HostState>, _level: i32, _ptr: i32, _len: i32| -> i32 {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if should_panic.load(Ordering::SeqCst) {
                        if let Some(period) = failed_tick_period {
                            caller.data_mut().tick_period_ms = period;
                        }
                        panic!("{HOST_PANIC_SENTINEL}");
                    }
                    host_abi::STATUS_OK
                },
            )
            .map(|_| ())
            .map_err(|e| format!("test linker setup: {e}"))
    }
}

async fn load_with_panicking_linker(
    runtime: &WasmRuntime,
    should_panic: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    failed_tick_period: Option<u64>,
) -> Result<Arc<ModuleEntry>, String> {
    runtime
        .get_or_load_with_linker(panicking_linker(should_panic, calls, failed_tick_period))
        .await
}

/// Initialization-only guest that rejects unless both Proxy-Wasm config
/// paths expose the exact configured non-UTF-8 bytes.
const RAW_CONFIG_GUEST: &str = r#"
    (module
      (import "env" "proxy_get_buffer_bytes"
        (func $get_buffer (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_get_configuration"
        (func $get_configuration (param i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32)
        (if (i32.ne (local.get 1) (i32.const 3)) (then (return (i32.const 0))))
        (if (i32.ne
              (call $get_buffer
                (i32.const 6) (i32.const 0) (i32.const -1)
                (i32.const 0) (i32.const 4))
              (i32.const 0))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load (i32.const 4)) (i32.const 3))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1024)) (i32.const 254))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1025)) (i32.const 1))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1026)) (i32.const 127))
          (then (return (i32.const 0))))
        (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32)
        (if (i32.ne (local.get 1) (i32.const 3)) (then (return (i32.const 0))))
        (if (i32.ne
              (call $get_buffer
                (i32.const 7) (i32.const 0) (i32.const -1)
                (i32.const 0) (i32.const 4))
              (i32.const 0))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load (i32.const 4)) (i32.const 3))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1024)) (i32.const 255))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1025)) (i32.const 0))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1026)) (i32.const 128))
          (then (return (i32.const 0))))
        (if (i32.ne
              (call $get_configuration (i32.const 0) (i32.const 4))
              (i32.const 0))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load (i32.const 4)) (i32.const 3))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1024)) (i32.const 255))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1025)) (i32.const 0))
          (then (return (i32.const 0))))
        (if (i32.ne (i32.load8_u (i32.const 1026)) (i32.const 128))
          (then (return (i32.const 0))))
        (i32.const 1))
      (func (export "proxy_abi_version_0_2_0"))
    )
    "#;

fn file_config(path: &std::path::Path) -> edgion_resources::plugin_configs::WasmConfig {
    let mut c = edgion_resources::plugin_configs::WasmConfig::default();
    c.source.file = Some(path.to_string_lossy().into_owned());
    c
}

/// Keep publication alive for synchronous lease/pool tests. A naked entry Arc
/// intentionally no longer preserves an idle pool after runtime retirement.
struct LoadedTestEntry {
    entry: Arc<ModuleEntry>,
    _runtime: WasmRuntime,
}
impl std::ops::Deref for LoadedTestEntry {
    type Target = Arc<ModuleEntry>;
    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}
impl std::fmt::Debug for LoadedTestEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.entry.fmt(f)
    }
}
async fn load(
    mgr: &WasmVmManager,
    config: &edgion_resources::plugin_configs::WasmConfig,
) -> Result<LoadedTestEntry, String> {
    let runtime = actual_runtime(mgr, config, CalloutGrant::none());
    let entry = runtime.get_or_load().await?;
    Ok(LoadedTestEntry {
        entry,
        _runtime: runtime,
    })
}

fn call_panic_site(lease: &mut VmLease, site: PanicSite) -> Result<(), String> {
    let phase = match site {
        PanicSite::ResponseHeaders => WasmPhase::ResponseHeaders,
        PanicSite::RequestBody => WasmPhase::RequestBody,
        PanicSite::ResponseBody => WasmPhase::ResponseBody,
        PanicSite::Tick => WasmPhase::Tick,
        _ => WasmPhase::RequestHeaders,
    };
    lease
        .state_mut()
        .begin_phase(phase, Vec::new(), Vec::new(), HashMap::new());
    match site {
        PanicSite::RequestContext | PanicSite::RequestHeaders => lease.call_on_request_headers(0).map(|_| ()),
        PanicSite::ResponseHeaders => lease.call_on_response_headers(0).map(|_| ()),
        PanicSite::RequestBody => lease.call_on_request_body(0, true).map(|_| ()),
        PanicSite::ResponseBody => lease.call_on_response_body(0, true).map(|_| ()),
        PanicSite::HttpCallResponse => lease.call_on_http_call_response(ROOT_CONTEXT_ID, 1, 0, 0),
        PanicSite::Tick => lease.call_on_tick(),
        PanicSite::Done | PanicSite::Delete => {
            let (ctx_id, _) = lease.call_on_request_headers_open(0)?;
            lease.close_context(ctx_id)
        }
        _ => unreachable!("initialization-only panic site"),
    }
}

fn assert_phase_trap_recorded(snapshotter: &metrics_util::debugging::Snapshotter, module: &str, phase: &str) {
    use metrics_util::debugging::DebugValue;

    let snapshot = snapshotter.snapshot().into_vec();
    let value = snapshot
        .iter()
        .find(|(key, _, _, _)| {
            key.key().name() == crate::observe::metrics::registry::names::WASM_TRAP_TOTAL
                && key
                    .key()
                    .labels()
                    .any(|label| label.key() == "module" && label.value() == module)
                && key
                    .key()
                    .labels()
                    .any(|label| label.key() == "phase" && label.value() == phase)
        })
        .map(|(_, _, _, value)| value)
        .expect("phase trap metric is registered");
    assert!(
        matches!(value, DebugValue::Counter(1)),
        "unexpected trap value: {value:?}"
    );
}

#[tokio::test]
async fn host_panics_during_initialization_are_redacted_and_never_published() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "host_panics_during_initialization_are_redacted_and_never_published",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "host_panics_during_initialization_are_redacted_and_never_published",
        || async {
            for site in [
                PanicSite::Instantiate,
                PanicSite::Initialize,
                PanicSite::Start,
                PanicSite::RootContext,
                PanicSite::VmStart,
                PanicSite::Configure,
            ] {
                let file = write_temp_wasm(&panic_guest(site));
                let config = file_config(file.path());
                let manager = WasmVmManager::new_for_test();
                let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
                let should_panic = Arc::new(AtomicBool::new(true));
                let calls = Arc::new(AtomicUsize::new(0));

                let error = load_with_panicking_linker(&runtime, should_panic, calls.clone(), None)
                    .await
                    .expect_err("host panic must fail eager initialization");

                assert!(error.ends_with(WASM_HOST_PANIC_ERROR), "{site:?}: {error}");
                assert!(!error.contains(HOST_PANIC_SENTINEL), "{site:?}: {error}");
                assert_eq!(calls.load(Ordering::SeqCst), 1, "{site:?}");
                assert!(runtime.try_get().is_none(), "{site:?} was published");
                let slot = runtime.state.slot.clone();
                assert!(slot.published.load_full().is_none(), "{site:?} published a candidate");
                assert!(
                    matches!(&*slot.state.lock().await, LoadState::Failed { .. }),
                    "{site:?}"
                );
            }
        },
    )
    .await;
}

#[tokio::test]
async fn runtime_host_panics_record_phase_traps_discard_and_recover() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "runtime_host_panics_record_phase_traps_discard_and_recover",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "runtime_host_panics_record_phase_traps_discard_and_recover",
        || async {
            use metrics_util::debugging::DebuggingRecorder;

            for (site, phase) in [
                (PanicSite::RequestContext, "request_headers"),
                (PanicSite::RequestHeaders, "request_headers"),
                (PanicSite::ResponseHeaders, "response_headers"),
                (PanicSite::RequestBody, "request_body"),
                (PanicSite::ResponseBody, "response_body"),
                (PanicSite::HttpCallResponse, "http_call_response"),
                (PanicSite::Tick, "tick"),
                (PanicSite::Done, "context_close"),
                (PanicSite::Delete, "context_close"),
            ] {
                let file = write_temp_wasm(&panic_guest(site));
                let config = file_config(file.path());
                let manager = WasmVmManager::new_for_test();
                let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
                let should_panic = Arc::new(AtomicBool::new(false));
                let calls = Arc::new(AtomicUsize::new(0));
                let entry = load_with_panicking_linker(&runtime, should_panic.clone(), calls, None)
                    .await
                    .expect("guest initializes while panic is disabled");
                assert_eq!(entry.pool_len(), 1, "{site:?} eager pool");

                should_panic.store(true, Ordering::SeqCst);
                let recorder = DebuggingRecorder::new();
                let snapshotter = recorder.snapshotter();
                {
                    let mut lease = entry.acquire(100).expect("warm lease");
                    let error = metrics::with_local_recorder(&recorder, || call_panic_site(&mut lease, site))
                        .expect_err("host panic must surface as an error");
                    assert_eq!(error, WASM_HOST_PANIC_ERROR, "{site:?}");
                    assert!(!error.contains(HOST_PANIC_SENTINEL), "{site:?}");
                }
                assert_phase_trap_recorded(&snapshotter, entry.metrics_label(), phase);
                assert_eq!(entry.pool_len(), 0, "{site:?} poisoned VM returned to pool");

                should_panic.store(false, Ordering::SeqCst);
                {
                    let mut fresh = entry.acquire(100).expect("fresh VM after discard");
                    call_panic_site(&mut fresh, site).expect("healthy follow-up call");
                }
                assert_eq!(entry.pool_len(), 1, "{site:?} healthy VM was not pooled");
                let reused = entry.acquire(100).expect("reuse healthy VM");
                assert_eq!(entry.pool_len(), 0, "{site:?} did not pop the healthy VM");
                drop(reused);
            }
        },
    )
    .await;
}

/// Regression (contract-01): two plugins sharing one module source but
/// carrying different `pluginConfig` must NOT alias the same cached
/// `ModuleEntry`. Before the fix both computed the source-only key and the
/// second received the first's configured entry (cross-route isolation
/// failure).
#[tokio::test]
async fn same_source_distinct_plugin_config_do_not_alias() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "same_source_distinct_plugin_config_do_not_alias",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "same_source_distinct_plugin_config_do_not_alias",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mut cfg_a = file_config(f.path());
            cfg_a.plugin_config = Some(WasmRawConfig::string("tenant-a"));
            let mut cfg_b = file_config(f.path());
            cfg_b.plugin_config = Some(WasmRawConfig::string("tenant-b"));

            let mgr = WasmVmManager::new_for_test();
            let entry_a = load(&mgr, &cfg_a).await.expect("loads a");
            let entry_b = load(&mgr, &cfg_b).await.expect("loads b");

            assert!(
                !Arc::ptr_eq(&entry_a, &entry_b),
                "distinct pluginConfig must not share a cached entry"
            );
            assert_eq!(entry_a.plugin_config, b"tenant-a");
            assert_eq!(entry_b.plugin_config, b"tenant-b");
        },
    )
    .await;
}

/// Regression (contract-01): reloading a plugin with only `pluginConfig`
/// changed lands on a new configured entry, so a configuration-only hot
/// reload is not silently a no-op.
#[tokio::test]
async fn plugin_config_only_reload_lands_new_entry() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "plugin_config_only_reload_lands_new_entry",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "plugin_config_only_reload_lands_new_entry",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mut cfg = file_config(f.path());
            cfg.plugin_config = Some(WasmRawConfig::string("v1"));
            let mgr = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&mgr, &cfg, CalloutGrant::none());
            let e1 = runtime.get_or_load().await.expect("loads v1");
            let e1b = runtime.get_or_load().await.expect("cached v1");
            assert!(Arc::ptr_eq(&e1, &e1b), "unchanged config shares the entry");

            cfg.plugin_config = Some(WasmRawConfig::string("v2"));
            let e2 = load(&mgr, &cfg).await.expect("loads v2");
            assert!(
                !Arc::ptr_eq(&e1, &e2),
                "pluginConfig-only reload must produce a new entry"
            );
            assert_eq!(e2.plugin_config, b"v2");
        },
    )
    .await;
}

#[tokio::test]
async fn plugin_drop_releases_publication_but_preserves_in_flight_lease() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "plugin_drop_releases_publication_but_preserves_in_flight_lease",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "plugin_drop_releases_publication_but_preserves_in_flight_lease",
        || async {
            let file = write_temp_wasm(MINI_GUEST);
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &file_config(file.path()), CalloutGrant::none());
            let state = runtime.state.clone();
            let entry = runtime.get_or_load().await.unwrap();
            let weak = Arc::downgrade(&entry);
            let mut lease = entry.acquire(100).unwrap();
            drop(entry);
            drop(runtime);
            assert!(state.slot.published.load_full().is_none());
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            assert!(lease.call_on_request_headers(0).is_ok());
            drop(lease);
            assert!(weak.upgrade().is_none());
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_data_metric_handle_follows_plugin_and_lease_lifetime() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "shared_data_metric_handle_follows_plugin_and_lease_lifetime",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "shared_data_metric_handle_follows_plugin_and_lease_lifetime",
        || async {
            use edgion_common::observe::{
                cardinality_guard::CardinalityMode,
                metrics_api::{RecorderConfig, TestRecorder},
            };
            let recorder = TestRecorder::new(RecorderConfig {
                service: "test",
                idle_timeout_secs: 600,
                histogram_buckets: &[],
                cardinality_cap: 10000,
                cardinality_mode: CardinalityMode::Enforce,
            })
            .unwrap();
            let file = write_temp_wasm(MINI_GUEST);
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &file_config(file.path()), CalloutGrant::none());
            let entry = recorder
                .with(|| {
                    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(runtime.get_or_load()))
                })
                .unwrap();
            let lease = entry.acquire(100).unwrap();
            drop(entry);
            drop(runtime);
            assert!(recorder.stats().gauge_owners >= 2);
            drop(lease);
            assert_eq!(recorder.stats().gauge_owners, 0);
        },
    )
    .await;
}

#[tokio::test]
async fn dropped_plugin_rejects_late_load_publication_and_tick_start() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "dropped_plugin_rejects_late_load_publication_and_tick_start",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "dropped_plugin_rejects_late_load_publication_and_tick_start",
        || async {
            let file = write_temp_wasm(TICK_GUEST);
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &file_config(file.path()), CalloutGrant::none());
            let state = runtime.state.clone();
            let guard = state.slot.state.lock().await;
            let late = state.clone();
            let (started, ready) = tokio::sync::oneshot::channel();
            let load = tokio::spawn(async move {
                let _ = started.send(());
                late.get_or_load_with_linker(add_production_host_abi).await
            });
            ready.await.unwrap();
            drop(runtime);
            drop(guard);
            assert_eq!(load.await.unwrap().unwrap_err(), "wasm plugin runtime stopped");
            assert!(state.slot.published.load_full().is_none());
            assert_eq!(state.tick_callouts.active_tick_tasks.load(Ordering::Acquire), 0);
        },
    )
    .await;
}

#[tokio::test]
async fn identical_sources_keep_plugin_grants_pools_and_shared_data_independent() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "identical_sources_keep_plugin_grants_pools_and_shared_data_independent",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "identical_sources_keep_plugin_grants_pools_and_shared_data_independent",
        || async {
            let file = write_temp_wasm(MINI_GUEST);
            let config = file_config(file.path());
            let manager = WasmVmManager::new_for_test();
            let grant_a = grant(vec!["https://a.example".to_string()], 100);
            let grant_b = grant(vec!["https://b.example".to_string()], 200);
            let a = actual_runtime(&manager, &config, grant_a.clone());
            let b = actual_runtime(&manager, &config, grant_b.clone());
            let entry_a = a.get_or_load().await.unwrap();
            let entry_b = b.get_or_load().await.unwrap();
            assert!(!Arc::ptr_eq(&entry_a, &entry_b));
            assert!(!Arc::ptr_eq(&entry_a.shared_data, &entry_b.shared_data));
            assert_eq!(entry_a.tick_callouts.grant, grant_a);
            assert_eq!(entry_b.tick_callouts.grant, grant_b);
            let lease = entry_a.acquire(100).unwrap();
            assert_eq!(entry_a.pool_len(), 0);
            assert_eq!(entry_b.pool_len(), 1);
            drop(a);
            assert!(Arc::ptr_eq(&b.get_or_load().await.unwrap(), &entry_b));
            assert_eq!(entry_b.tick_callouts.grant, grant_b);
            drop(lease);
        },
    )
    .await;
}

#[tokio::test]
async fn recreated_plugin_reads_replaced_file_and_owns_new_policy() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "recreated_plugin_reads_replaced_file_and_owns_new_policy",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "recreated_plugin_reads_replaced_file_and_owns_new_policy",
        || async {
            let file = write_temp_wasm(MINI_GUEST);
            let config = file_config(file.path());
            let manager = WasmVmManager::new_for_test();
            let old = actual_runtime(&manager, &config, CalloutGrant::none());
            let old_entry = old.get_or_load().await.unwrap();
            std::fs::write(file.path(), wat::parse_str(TRAP_GUEST).unwrap()).unwrap();
            let new = actual_runtime(&manager, &config, CalloutGrant::none());
            let new_entry = new.get_or_load().await.unwrap();
            let mut lease = new_entry.acquire(100).unwrap();
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            assert!(lease.call_on_request_headers(0).is_err());
            assert!(!Arc::ptr_eq(&old_entry, &new_entry));
        },
    )
    .await;
}

#[tokio::test]
async fn real_guest_receives_exact_non_utf8_vm_and_plugin_config_bytes() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "real_guest_receives_exact_non_utf8_vm_and_plugin_config_bytes",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "real_guest_receives_exact_non_utf8_vm_and_plugin_config_bytes",
        || async {
            let file = write_temp_wasm(RAW_CONFIG_GUEST);
            let mut config = file_config(file.path());
            config.vm_config = Some(WasmRawConfig::base64("/gF/"));
            config.plugin_config = Some(WasmRawConfig::base64("/wCA"));

            let manager = WasmVmManager::new_for_test();
            let entry = load(&manager, &config)
                .await
                .expect("guest accepts exact VM, plugin, and legacy configuration bytes");
            assert_eq!(entry.vm_config, [0xfe, 0x01, 0x7f]);
            assert_eq!(entry.plugin_config, [0xff, 0x00, 0x80]);
        },
    )
    .await;
}

#[tokio::test]
async fn gateway_rejects_invalid_raw_config_without_assuming_controller_validation() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "gateway_rejects_invalid_raw_config_without_assuming_controller_validation",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "gateway_rejects_invalid_raw_config_without_assuming_controller_validation",
        || async {
            let file = write_temp_wasm(MINI_GUEST);
            let mut config = file_config(file.path());
            config.plugin_config = Some(WasmRawConfig::base64("YR=="));

            let error = load(&WasmVmManager::new_for_test(), &config)
                .await
                .expect_err("non-canonical base64 must be rejected at the Gateway boundary");
            assert_eq!(error, "pluginConfig must be canonical padded Standard base64");
        },
    )
    .await;
}

#[tokio::test]
async fn load_from_file_and_lease() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "load_from_file_and_lease",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "load_from_file_and_lease",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.expect("loads");
            let mut lease = entry.acquire(100).expect("lease");
            // Instance is initialized: request-headers hook is callable.
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            let action = lease.call_on_request_headers(0).expect("call ok");
            assert_eq!(action, 0);
        },
    )
    .await;
}

#[tokio::test]
async fn request_body_hook_probe_and_call() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "request_body_hook_probe_and_call",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "request_body_hook_probe_and_call",
        || async {
            // Guest exports proxy_on_request_body returning Action::Continue (0).
            const REQUEST_BODY_GUEST: &str = r#"
                (module
                  (memory (export "memory") 1)
                  (global $heap (mut i32) (i32.const 1024))
                  (func (export "proxy_on_memory_allocate") (param i32) (result i32)
                    global.get $heap)
                  (func (export "proxy_on_context_create") (param i32 i32))
                  (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
                  (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
                  (func (export "proxy_on_request_body") (param i32 i32 i32) (result i32) (i32.const 0))
                  (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
                  (func (export "proxy_on_delete") (param i32))
                )
                "#;
            let f = write_temp_wasm(REQUEST_BODY_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.expect("loads");
            assert!(entry.has_request_body_hook());
            let mut lease = entry.acquire(1000).expect("lease");
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
            let action = lease.call_on_request_body(3, true).expect("call ok");
            assert_eq!(action, 0);
        },
    )
    .await;
}

#[tokio::test]
async fn pool_reuses_instance() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "pool_reuses_instance",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "pool_reuses_instance",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.unwrap();
            {
                let _lease = entry.acquire(100).unwrap();
            } // returned to pool on drop
            assert_eq!(entry.pool_len(), 1);
            let _l2 = entry.acquire(100).unwrap();
            assert_eq!(entry.pool_len(), 0); // popped, not re-created
        },
    )
    .await;
}

#[tokio::test]
async fn idle_store_releases_phase_payloads_without_resetting_persistent_state() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "idle_store_releases_phase_payloads_without_resetting_persistent_state",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "idle_store_releases_phase_payloads_without_resetting_persistent_state",
        || async {
            let file = write_temp_wasm(MINI_GUEST);
            let manager = WasmVmManager::new_for_test();
            let entry = load(&manager, &file_config(file.path())).await.unwrap();
            let shared = entry.shared_data.clone();
            let mut lease = entry.acquire(100).unwrap();
            let state = lease.state_mut();
            state.vm_config = b"vm".to_vec();
            state.plugin_config = b"plugin".to_vec().into();
            state.tick_period_ms = 1234;
            state.frame.request_headers = vec![("authorization".into(), "test-credential".into())];
            state.frame.response_headers = vec![("set-cookie".into(), "test-cookie".into())];
            state.frame.properties.insert("request.path".into(), "/private".into());
            state.frame.request_body = vec![b'x'; 128 * 1024];
            state.frame.body_chunk = bytes::Bytes::from_static(b"response");
            state.frame.log_lines.push((1, "test message".into()));
            state.arm_dispatch(Arc::from(vec!["https://example.com:443".into()]), 50);
            state.frame.continue_requested = true;
            drop(lease);

            // Inspect before acquire/begin_phase could hide an idle retention defect.
            let pool = entry.pool.lock();
            let state = pool[0].store.data();
            assert_eq!(state.frame.request_body.capacity(), 0);
            assert_eq!(state.frame.request_headers.capacity(), 0);
            assert_eq!(state.frame.response_headers.capacity(), 0);
            assert_eq!(state.frame.properties.capacity(), 0);
            assert_eq!(state.frame.log_lines.capacity(), 0);
            assert_eq!(state.frame.pending_ops.capacity(), 0);
            assert_eq!(state.frame.pending_callouts.capacity(), 0);
            assert!(state.frame.body_chunk.is_empty());
            assert!(!state.frame.dispatch_enabled);
            assert!(!state.frame.continue_requested);
            assert!(state.frame.callout_allowlist.is_none());
            assert_eq!(state.vm_config, b"vm");
            assert_eq!(&state.plugin_config[..], b"plugin");
            assert_eq!(state.tick_period_ms, 1234);
            assert!(Arc::ptr_eq(&state.shared_data, &shared));
        },
    )
    .await;
}

#[tokio::test]
async fn repeated_test_manager_drop_releases_engine_pooling_state() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "repeated_test_manager_drop_releases_engine_pooling_state",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "repeated_test_manager_drop_releases_engine_pooling_state",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            for _ in 0..24 {
                let mgr = WasmVmManager::new_for_test();
                assert!(mgr.epoch_ticker.is_some());
                let entry = load(&mgr, &file_config(f.path())).await.expect("manager loads module");
                let mut lease = entry.acquire(100).expect("manager leases VM");
                lease
                    .state_mut()
                    .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
                assert_eq!(lease.call_on_request_headers(0).expect("guest hook runs"), 0);
            }

            // A manager created after the churn must still reserve pooling
            // mappings and run a module; leaked ticker Engine clones used to make
            // this eventually fail after the wider suite created many managers.
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.expect("final manager loads");
            let mut lease = entry.acquire(100).expect("final manager leases VM");
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            assert_eq!(lease.call_on_request_headers(0).expect("final guest hook runs"), 0);
        },
    )
    .await;
}

#[tokio::test]
async fn abandoned_open_request_context_is_not_returned_to_pool() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "abandoned_open_request_context_is_not_returned_to_pool",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "abandoned_open_request_context_is_not_returned_to_pool",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.unwrap();
            {
                let mut lease = entry.acquire(100).expect("lease");
                lease
                    .state_mut()
                    .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
                lease.call_on_request_headers_open(0).expect("open request context");
            }

            assert_eq!(
                entry.pool_len(),
                0,
                "cancellation with an open guest context must discard the VM"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn closed_request_context_returns_vm_to_pool_for_reuse() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "closed_request_context_returns_vm_to_pool_for_reuse",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "closed_request_context_returns_vm_to_pool_for_reuse",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.unwrap();
            {
                let mut lease = entry.acquire(100).expect("lease");
                lease
                    .state_mut()
                    .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
                let (ctx_id, _) = lease.call_on_request_headers_open(0).expect("open request context");
                lease.close_context(ctx_id).expect("close request context");
            }

            assert_eq!(entry.pool_len(), 1, "fully closed guest context is reusable");
            let _reused = entry.acquire(100).expect("reacquire pooled VM");
            assert_eq!(entry.pool_len(), 0, "reacquire must pop the pooled VM");
        },
    )
    .await;
}

#[tokio::test]
async fn close_context_trap_discards_vm() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "close_context_trap_discards_vm",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "close_context_trap_discards_vm",
        || async {
            let wat = MINI_GUEST.replace(
                "(func (export \"proxy_on_done\") (param i32) (result i32) (i32.const 1))",
                "(func (export \"proxy_on_done\") (param i32) (result i32) unreachable)",
            );
            let f = write_temp_wasm(&wat);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.unwrap();
            {
                let mut lease = entry.acquire(100).expect("lease");
                lease
                    .state_mut()
                    .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
                let (ctx_id, _) = lease.call_on_request_headers_open(0).expect("open request context");
                assert!(lease.close_context(ctx_id).is_err(), "close trap must surface");
            }

            assert_eq!(entry.pool_len(), 0, "close trap must discard the VM");
        },
    )
    .await;
}

#[tokio::test]
async fn second_open_request_context_poisons_lease() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "second_open_request_context_poisons_lease",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "second_open_request_context_poisons_lease",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.unwrap();
            {
                let mut lease = entry.acquire(100).expect("lease");
                lease
                    .state_mut()
                    .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
                lease.call_on_request_headers_open(0).expect("first open");
                assert!(lease.call_on_request_headers_open(0).is_err(), "second open must fail");
            }

            assert_eq!(entry.pool_len(), 0, "duplicate open must discard the VM");
        },
    )
    .await;
}

#[tokio::test]
async fn poisoned_lease_not_returned() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "poisoned_lease_not_returned",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "poisoned_lease_not_returned",
        || async {
            let f = write_temp_wasm(TRAP_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.unwrap();
            {
                let mut lease = entry.acquire(100).unwrap();
                lease
                    .state_mut()
                    .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
                let error = lease
                    .call_on_request_headers(0)
                    .expect_err("trap must surface as error");
                assert!(
                    error.starts_with("proxy_on_request_headers trapped:"),
                    "ordinary Wasmtime trap shape changed: {error}"
                );
                assert!(
                    error.contains("wasm backtrace:"),
                    "ordinary trap lost its shape: {error}"
                );
                assert_ne!(error, WASM_HOST_PANIC_ERROR);
                // call_on_request_headers poisons on trap internally
            }
            assert_eq!(entry.pool_len(), 0, "trapped instance must be dropped");
            // A fresh lease still works (new instance from InstancePre):
            assert!(entry.acquire(100).is_ok());
        },
    )
    .await;
}

/// A pooled instance can sit idle in `ModuleEntry::pool` for an
/// arbitrary amount of time between requests while the background
/// ticker keeps advancing the engine epoch. The epoch deadline armed at
/// `acquire` time must not be the only arm point: it must be re-armed at
/// the start of each hook call, otherwise a lease built on an instance
/// that sat idle past its original deadline (but still well within the
/// per-hook timeout budget) traps on its next hook call regardless of
/// guest speed.
#[tokio::test]
async fn lease_survives_idle_gap_between_hook_calls() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "lease_survives_idle_gap_between_hook_calls",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "lease_survives_idle_gap_between_hook_calls",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.unwrap();
            let mut lease = entry.acquire(30).expect("lease");
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            let action = lease.call_on_request_headers(0).expect("first call ok");
            assert_eq!(action, 0);
            // Sleep well past the deadline armed at acquire() (30ms budget / 10ms
            // tick + 2 slack ticks = 5 ticks = 50ms); 120ms clears it with margin.
            std::thread::sleep(Duration::from_millis(120));
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            let action = lease
                .call_on_request_headers(0)
                .expect("second call must not trap on a stale deadline");
            assert_eq!(action, 0);
        },
    )
    .await;
}

#[tokio::test]
async fn sha256_mismatch_rejected() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "sha256_mismatch_rejected",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "sha256_mismatch_rejected",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mut config = file_config(f.path());
            config.sha256 = Some("0".repeat(64));
            let mgr = WasmVmManager::new_for_test();
            let err = load(&mgr, &config).await.unwrap_err();
            assert!(err.contains("sha256"), "got: {err}");
        },
    )
    .await;
}

/// Regression (contract-04): the per-resource `sha256` pin is enforced even
/// against a warm cache. Two plugins share one file source but pin different
/// digests; the mismatching one must fail rather than receive the first
/// plugin's already-loaded (verified) bytes on a cache hit.
#[tokio::test]
async fn sha256_pin_enforced_on_warm_cache_hit() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "sha256_pin_enforced_on_warm_cache_hit",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "sha256_pin_enforced_on_warm_cache_hit",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let bytes = std::fs::read(f.path()).unwrap();
            let correct = hex::encode(Sha256::digest(&bytes));

            let mut good = file_config(f.path());
            good.sha256 = Some(correct.clone());

            // Same file source, contradictory pin.
            let mut bad = file_config(f.path());
            bad.sha256 = Some("0".repeat(64));

            let mgr = WasmVmManager::new_for_test();
            // Warm the cache with the correct pin.
            let runtime = actual_runtime(&mgr, &good, CalloutGrant::none());
            let verified = runtime.get_or_load().await.expect("correct pin loads");

            // The mismatching config must still fail even though the bytes are warm.
            let err = load(&mgr, &bad).await.unwrap_err();
            assert!(err.contains("sha256"), "warm cache must not bypass the pin; got: {err}");

            // Digest comparison remains case-insensitive for independent plugins.
            let mut good2 = file_config(f.path());
            good2.sha256 = Some(correct.to_ascii_uppercase());
            let uppercase = load(&mgr, &good2).await.expect("uppercase pin loads");
            assert!(!Arc::ptr_eq(&verified, &uppercase));
        },
    )
    .await;
}

#[tokio::test]
async fn missing_file_is_load_error() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "missing_file_is_load_error",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "missing_file_is_load_error",
        || async {
            let mut config = edgion_resources::plugin_configs::WasmConfig::default();
            config.source.file = Some("/nonexistent/definitely-missing.wasm".to_string());
            let mgr = WasmVmManager::new_for_test();
            assert!(load(&mgr, &config).await.is_err());
        },
    )
    .await;
}

/// ABI check branch 1: a supported version export loads fine.
#[tokio::test]
async fn abi_supported_version_loads() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "abi_supported_version_loads",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "abi_supported_version_loads",
        || async {
            let wat = MINI_GUEST.replace(
                    "(func (export \"proxy_on_context_create\") (param i32 i32))",
                    "(func (export \"proxy_on_context_create\") (param i32 i32))\n      (func (export \"proxy_abi_version_0_2_1\"))",
                );
            let f = write_temp_wasm(&wat);
            let mgr = WasmVmManager::new_for_test();
            assert!(load(&mgr, &file_config(f.path())).await.is_ok());
        },
    ).await;
}

/// An unsupported proxy_abi_version_* export fails with the same bounded
/// compatibility error used for all untrusted module-shape failures.
#[tokio::test]
async fn abi_unsupported_version_rejected() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "abi_unsupported_version_rejected",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "abi_unsupported_version_rejected",
        || async {
            let wat = MINI_GUEST.replace(
                    "(func (export \"proxy_on_context_create\") (param i32 i32))",
                    "(func (export \"proxy_on_context_create\") (param i32 i32))\n      (func (export \"proxy_abi_version_0_1_0\"))",
                );
            let f = write_temp_wasm(&wat);
            let mgr = WasmVmManager::new_for_test();
            let err = load(&mgr, &file_config(f.path())).await.unwrap_err();
            assert_eq!(err, "wasm compatibility check failed");
        },
    ).await;
}

#[tokio::test]
async fn unknown_proxy_hook_export_is_rejected_before_compilation() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "unknown_proxy_hook_export_is_rejected_before_compilation",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "unknown_proxy_hook_export_is_rejected_before_compilation",
        || async {
            let wat = MINI_GUEST.replace(
                    "(func (export \"proxy_on_request_headers\") (param i32 i32 i32) (result i32) (i32.const 0))",
                    "(func (export \"proxy_on_request_headers\") (param i32 i32 i32) (result i32) (i32.const 0))\n      (func (export \"proxy_on_request_header\") (param i32 i32 i32) (result i32) (i32.const 0))",
                );
            let file = write_temp_wasm(&wat);
            let manager = WasmVmManager::new_for_test();
            let error = load(&manager, &file_config(file.path())).await.unwrap_err();
            assert_eq!(error, "wasm compatibility check failed");
        },
    ).await;
}

/// ABI check branch 3: no version export at all loads with a WARN
/// (hand-written WAT test modules and non-standard guests).
#[tokio::test]
async fn abi_missing_version_allowed() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "abi_missing_version_allowed",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "abi_missing_version_allowed",
        || async {
            let f = write_temp_wasm(MINI_GUEST);
            let mgr = WasmVmManager::new_for_test();
            assert!(load(&mgr, &file_config(f.path())).await.is_ok());
        },
    )
    .await;
}

#[test]
fn reference_guest_matches_host_compatibility_profile() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/wasm/artifacts/header_demo.wasm");
    let bytes = std::fs::read(path).expect("committed reference guest must be readable");
    let report = edgion_wasm::analyze(&bytes);
    assert!(report.compatible, "{:#?}\n{:#?}", report.errors, report.imports);
    assert!(report.hooks.iter().any(|hook| {
        hook.name == "proxy_on_request_headers" && hook.support == edgion_wasm::HookSupport::Implemented
    }));
    assert!(report.hooks.iter().any(|hook| {
        hook.name == "proxy_on_request_trailers" && hook.support == edgion_wasm::HookSupport::KnownNotInvoked
    }));
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.code == edgion_wasm::CompatibilityCode::KnownNotInvokedHook));
}

#[tokio::test]
async fn refresh_claim_is_released_when_task_is_aborted() {
    let slot = Arc::new(LoadSlot::new(true));
    let claim = slot.try_claim_refresh().expect("first task claims refresh");
    let task = tokio::spawn(async move {
        let _claim = claim;
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    task.abort();
    let _ = task.await;

    assert!(
        !slot.refresh_in_flight.load(Ordering::Acquire),
        "aborting the task must release the refresh claim"
    );
    assert!(
        slot.try_claim_refresh().is_some(),
        "a later refresh must be able to claim the slot"
    );
}

#[test]
fn non_always_slot_disables_refresh_without_runtime_config_parsing() {
    let slot = LoadSlot::new(false);
    assert_eq!(
        next_oci_refresh_deadline(&slot, true),
        u64::MAX,
        "the precomputed slot policy alone disables refresh"
    );
}

/// url fetch: the resolved auth header is sent as `Authorization`.
#[tokio::test]
async fn url_fetch_sends_resolved_auth_header() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "url_fetch_sends_resolved_auth_header",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "url_fetch_sends_resolved_auth_header",
        || async {
            use wiremock::matchers::{header, method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            let bytes = wat::parse_str(MINI_GUEST).unwrap();
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/m.wasm"))
                .and(header("authorization", "Bearer repo-token"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
                .mount(&server)
                .await;

            let mut config = edgion_resources::plugin_configs::WasmConfig::default();
            config.source.url = Some(format!("{}/m.wasm", server.uri()));
            config.source.fetch = Some(edgion_resources::plugin_configs::WasmFetchSpec {
                auth_header_secret_ref: Some(edgion_resources::plugin_configs::WasmAuthHeaderSecretRef {
                    secret: edgion_resources::resources::common::SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "repo-token".to_string(),
                        namespace: None,
                    },
                    key: "authorization".to_string(),
                }),
                resolved_auth_header: Some("Bearer repo-token".to_string()),
                timeout_ms: 5000,
                ..Default::default()
            });

            let mgr = WasmVmManager::new_for_test();
            assert!(load(&mgr, &config).await.is_ok());
        },
    )
    .await;
}

/// url fetch: an unresolved auth header secret fails the load (no
/// silent unauthenticated fetch).
#[tokio::test]
async fn url_fetch_unresolved_auth_secret_fails_load() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "url_fetch_unresolved_auth_secret_fails_load",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "url_fetch_unresolved_auth_secret_fails_load",
        || async {
            let mut config = edgion_resources::plugin_configs::WasmConfig::default();
            config.source.url = Some("http://localhost:1/m.wasm".to_string());
            config.source.fetch = Some(edgion_resources::plugin_configs::WasmFetchSpec {
                auth_header_secret_ref: Some(edgion_resources::plugin_configs::WasmAuthHeaderSecretRef {
                    secret: edgion_resources::resources::common::SecretObjectReference {
                        group: None,
                        kind: None,
                        name: "repo-token".to_string(),
                        namespace: None,
                    },
                    key: "authorization".to_string(),
                }),
                resolved_auth_header: None,
                timeout_ms: 500,
                ..Default::default()
            });
            let mgr = WasmVmManager::new_for_test();
            let err = load(&mgr, &config).await.unwrap_err();
            assert!(err.contains("auth header secret not resolved"), "got: {err}");
        },
    )
    .await;
}

#[tokio::test]
async fn url_fetch_transport_error_does_not_expose_configured_url() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve an unused local port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let configured_url =
        format!("http://alice:password@127.0.0.1:{port}/private/module.wasm?token=repo-secret#fragment");

    let error = fetch_url_bytes(&configured_url, None, Some(&actual_url_client(&configured_url, None)))
        .await
        .unwrap_err();
    assert!(error.starts_with("fetch wasm url: "), "got: {error}");
    for secret in [
        configured_url.as_str(),
        "alice",
        "password",
        "private",
        "module.wasm",
        "repo-secret",
        "fragment",
    ] {
        assert!(!error.contains(secret), "error leaked {secret}: {error}");
    }
}

/// url fetch retry: first attempt 503, second 200 → load succeeds and
/// exactly two requests hit the server.
#[tokio::test]
async fn url_fetch_retries_5xx_then_succeeds() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "url_fetch_retries_5xx_then_succeeds",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "url_fetch_retries_5xx_then_succeeds",
        || async {
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            let bytes = wat::parse_str(MINI_GUEST).unwrap();
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/m.wasm"))
                .respond_with(ResponseTemplate::new(503))
                .up_to_n_times(1)
                .with_priority(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/m.wasm"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
                .with_priority(2)
                .mount(&server)
                .await;

            let mut config = edgion_resources::plugin_configs::WasmConfig::default();
            config.source.url = Some(format!("{}/m.wasm", server.uri()));
            config.source.fetch = Some(edgion_resources::plugin_configs::WasmFetchSpec {
                timeout_ms: 5000,
                retry: Some(edgion_resources::plugin_configs::WasmFetchRetry {
                    attempts: 3,
                    backoff_ms: 1,
                }),
                ..Default::default()
            });

            let mgr = WasmVmManager::new_for_test();
            assert!(load(&mgr, &config).await.is_ok());
            assert_eq!(server.received_requests().await.unwrap().len(), 2);
        },
    )
    .await;
}

#[tokio::test]
async fn url_retry_backoff_releases_permit_for_another_fetch() {
    use parking_lot::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Notify, Semaphore};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let order = Arc::new(StdMutex::new(Vec::new()));
    let first_a_seen = Arc::new(Notify::new());
    let a_calls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/a.wasm"))
        .respond_with({
            let order = order.clone();
            let first_a_seen = first_a_seen.clone();
            let a_calls = a_calls.clone();
            move |_: &wiremock::Request| {
                let call = a_calls.fetch_add(1, Ordering::SeqCst);
                order.lock().push(if call == 0 { "a1" } else { "a2" });
                if call == 0 {
                    first_a_seen.notify_one();
                    ResponseTemplate::new(503)
                } else {
                    ResponseTemplate::new(200).set_body_bytes(b"a".to_vec())
                }
            }
        })
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/b.wasm"))
        .respond_with({
            let order = order.clone();
            move |_: &wiremock::Request| {
                order.lock().push("b");
                ResponseTemplate::new(200).set_body_bytes(b"b".to_vec())
            }
        })
        .mount(&server)
        .await;

    let retry = edgion_resources::plugin_configs::WasmFetchSpec {
        timeout_ms: 5000,
        retry: Some(edgion_resources::plugin_configs::WasmFetchRetry {
            attempts: 2,
            backoff_ms: 500,
        }),
        ..Default::default()
    };
    let permits = Arc::new(Semaphore::new(1));
    let url_a = format!("{}/a.wasm", server.uri());
    let url_b = format!("{}/b.wasm", server.uri());
    let permits_a = permits.clone();
    let a = tokio::spawn(async move {
        fetch_url_bytes_with_permits(
            &permits_a,
            &url_a,
            Some(&retry),
            Some(&actual_url_client(&url_a, Some(&retry))),
        )
        .await
    });
    first_a_seen.notified().await;
    let permits_b = permits.clone();
    let b = tokio::spawn(async move {
        fetch_url_bytes_with_permits(&permits_b, &url_b, None, Some(&actual_url_client(&url_b, None))).await
    });

    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), b)
            .await
            .expect("B completes while A is backing off")
            .unwrap()
            .unwrap(),
        b"b"
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), a)
            .await
            .expect("A completes its retry")
            .unwrap()
            .unwrap(),
        b"a"
    );
    assert_eq!(&*order.lock(), &["a1", "b", "a2"]);
}

/// url fetch retry: 4xx is definitive — exactly one request, no retry.
#[tokio::test]
async fn url_fetch_does_not_retry_4xx() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "url_fetch_does_not_retry_4xx",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "url_fetch_does_not_retry_4xx",
        || async {
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/m.wasm"))
                .respond_with(ResponseTemplate::new(401))
                .mount(&server)
                .await;

            let mut config = edgion_resources::plugin_configs::WasmConfig::default();
            let configured_url = format!("{}/m.wasm?token=repo-secret#fragment", server.uri());
            config.source.url = Some(configured_url.clone());
            config.source.fetch = Some(edgion_resources::plugin_configs::WasmFetchSpec {
                timeout_ms: 5000,
                retry: Some(edgion_resources::plugin_configs::WasmFetchRetry {
                    attempts: 3,
                    backoff_ms: 1,
                }),
                ..Default::default()
            });

            let mgr = WasmVmManager::new_for_test();
            let err = load(&mgr, &config).await.unwrap_err();
            assert!(err.contains("status 401"), "got: {err}");
            assert!(!err.contains(&configured_url), "got: {err}");
            assert!(!err.contains("repo-secret"), "got: {err}");
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                1,
                "4xx must not be retried"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn preload_claim_is_single_flight_and_respects_retry_deadline() {
    let manager = WasmVmManager::new_for_test();
    let runtime = actual_runtime(&manager, &WasmConfig::default(), CalloutGrant::none());
    let state = &runtime.state;
    let claim = state.prepare_preload().expect("cold runtime admits preload");
    let mut callers = tokio::task::JoinSet::new();
    for _ in 0..64 {
        let state = state.clone();
        callers.spawn(async move { state.prepare_preload().is_none() });
    }
    while let Some(result) = callers.join_next().await {
        assert!(result.unwrap());
    }
    drop(claim);
    state.slot.load_retry_due_ms.store(u64::MAX, Ordering::Release);
    assert!(state.prepare_preload().is_none());
    state.slot.load_retry_due_ms.store(0, Ordering::Release);
    assert!(state.prepare_preload().is_some());
}

#[tokio::test]
async fn preload_is_single_flight_and_respects_negative_cache_deadline() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "preload_is_single_flight_and_respects_negative_cache_deadline",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "preload_is_single_flight_and_respects_negative_cache_deadline",
        || async {
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            let bytes = wat::parse_str(MINI_GUEST).unwrap();
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/ok.wasm"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_delay(Duration::from_millis(250))
                        .set_body_bytes(bytes),
                )
                .mount(&server)
                .await;
            let manager = Arc::new(WasmVmManager::new_for_test());

            let mut ok = WasmConfig::default();
            ok.source.url = Some(format!("{}/ok.wasm", server.uri()));
            let ok_runtime = actual_runtime(&manager, &ok, CalloutGrant::none());
            for _ in 0..64 {
                ok_runtime.spawn_preload();
            }
            ok_runtime.wait_for_preload_for_test().await;
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                1,
                "repeated sync misses must create one preload request"
            );

            server.reset().await;
            Mock::given(method("GET"))
                .and(path("/bad.wasm"))
                .respond_with(ResponseTemplate::new(400))
                .mount(&server)
                .await;
            let mut bad = WasmConfig::default();
            bad.source.url = Some(format!("{}/bad.wasm", server.uri()));
            let bad_runtime = actual_runtime(&manager, &bad, CalloutGrant::none());

            for _ in 0..64 {
                bad_runtime.spawn_preload();
            }
            bad_runtime.wait_for_preload_for_test().await;
            assert_eq!(server.received_requests().await.unwrap().len(), 1);

            // During the negative-cache window no task (and therefore no request)
            // is created, even if a response-body hook reports the miss per chunk.
            for _ in 0..64 {
                bad_runtime.spawn_preload();
            }
            assert_eq!(server.received_requests().await.unwrap().len(), 1);

            bad_runtime.backdate_load_retry_for_test();
            for _ in 0..64 {
                bad_runtime.spawn_preload();
            }
            bad_runtime.wait_for_preload_for_test().await;
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                2,
                "an expired negative-cache deadline permits exactly one retry"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn aborted_preload_releases_claim_for_retry() {
    let slot = Arc::new(LoadSlot::new(false));
    let claim = slot.try_claim_preload().expect("first preload claims slot");
    let task = tokio::spawn(async move {
        let _claim = claim;
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    task.abort();
    let _ = task.await;

    assert!(
        !slot.preload_in_flight.load(Ordering::Acquire),
        "aborting preload must release its claim"
    );
    assert!(
        slot.try_claim_preload().is_some(),
        "a later preload must be able to retry"
    );
}

#[tokio::test]
async fn engine_unavailable_preload_creates_no_task() {
    let manager = Arc::new(WasmVmManager::new_with_engine_result(Err(
        "wasm engine unavailable: test".to_string(),
    )));
    let mut config = WasmConfig::default();
    config.source.inline = Some("AAAA".to_string());
    let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
    runtime.spawn_preload();
    assert!(runtime.try_get().is_none());
    assert!(!runtime.state.slot.preload_in_flight.load(Ordering::Acquire));
}

#[tokio::test]
async fn bounded_file_reader_accepts_exact_limit() {
    let expected = vec![b'x'; 32];
    let actual = read_open_file_bounded(std::io::Cursor::new(expected.clone()), 32, 32)
        .await
        .expect("a file exactly at the cap is accepted");
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn file_fetch_fast_rejects_sparse_metadata_over_limit() {
    let file = tempfile::NamedTempFile::new().expect("temporary file");
    file.as_file()
        .set_len(MAX_MODULE_BYTES as u64 + 1)
        .expect("create sparse oversized file");

    let error = read_file_bounded(file.path()).await.unwrap_err();
    assert_eq!(error, module_too_large_error());
    assert!(!error.contains(&file.path().display().to_string()));
}

#[tokio::test]
async fn bounded_file_reader_rejects_actual_bytes_over_metadata_hint() {
    // The initial capacity represents a metadata length observed before a
    // concurrent writer grew the already-open file. The read limit remains
    // authoritative and detects the extra byte.
    let error = read_open_file_bounded(std::io::Cursor::new(vec![b'x'; 33]), 32, 32)
        .await
        .unwrap_err();
    assert!(matches!(error, BoundedFileReadError::TooLarge));
}

/// url fetch: a body exceeding the module cap is rejected by the streaming
/// counter rather than buffered whole. The helper counts actually-streamed
/// bytes, so the same rejection holds for absent/false/chunked
/// Content-Length.
#[tokio::test]
async fn url_fetch_rejects_oversized_body_at_the_cap() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "url_fetch_rejects_oversized_body_at_the_cap",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "url_fetch_rejects_oversized_body_at_the_cap",
        || async {
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            let server = MockServer::start().await;
            let big = vec![b'x'; MAX_MODULE_BYTES + 1];
            Mock::given(method("GET"))
                .and(path("/m.wasm"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
                .mount(&server)
                .await;

            let mut config = edgion_resources::plugin_configs::WasmConfig::default();
            config.source.url = Some(format!("{}/m.wasm", server.uri()));
            config.source.fetch = Some(edgion_resources::plugin_configs::WasmFetchSpec {
                timeout_ms: 30000,
                ..Default::default()
            });

            let mgr = WasmVmManager::new_for_test();
            let err = load(&mgr, &config).await.unwrap_err();
            assert!(err.contains("body exceeds"), "got: {err}");
        },
    )
    .await;
}

/// Shared-data roundtrip through the guest ABI: the request-headers
/// hook writes `k=hello` via proxy_set_shared_data, reads it back via
/// proxy_get_shared_data, and returns the value size it saw.
const SHARED_GUEST: &str = r#"
    (module
      (import "env" "proxy_set_shared_data"
        (func $set_sd (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_get_shared_data"
        (func $get_sd (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "k")
      (data (i32.const 16) "hello")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (drop (call $set_sd (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 5) (i32.const 0)))
        (drop (call $get_sd (i32.const 0) (i32.const 1) (i32.const 256) (i32.const 260) (i32.const 264)))
        (i32.load (i32.const 260)))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

/// Tick guest: on_configure requests a 100ms tick period; every
/// proxy_on_tick overwrites shared key `t` (each set bumps the CAS, so
/// the host can observe tick liveness through the CAS counter).
const TICK_GUEST: &str = r#"
    (module
      (import "env" "proxy_set_tick_period_milliseconds"
        (func $set_tick (param i32) (result i32)))
      (import "env" "proxy_set_shared_data"
        (func $set_sd (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "t")
      (data (i32.const 16) "x")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32)
        (drop (call $set_tick (i32.const 100)))
        (i32.const 1))
      (func (export "proxy_on_tick") (param i32)
        (drop (call $set_sd (i32.const 0) (i32.const 1) (i32.const 16) (i32.const 1) (i32.const 0))))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

const TICK_CALLOUT_PANIC_GUEST: &str = r#"
    (module
      (import "env" "proxy_log"
        (func $panic_import (param i32 i32 i32) (result i32)))
      (import "env" "proxy_http_call"
        (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "proxy_on_memory_allocate") (param i32) (result i32) (i32.const 1024))
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_tick") (param i32)
        (drop (call $dispatch
          (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)
          (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0))))
      (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32)
        (drop (call $panic_import (i32.const 0) (i32.const 0) (i32.const 0))))
      (func (export "proxy_abi_version_0_2_0"))
    )
    "#;

fn panicking_tick_callout_linker(
    should_panic: Arc<AtomicBool>,
    callback_calls: Arc<AtomicUsize>,
    dispatch_calls: Arc<AtomicUsize>,
) -> impl FnOnce(&mut Linker<HostState>) -> Result<(), String> + Send {
    move |linker| {
        panicking_linker(should_panic, callback_calls, Some(0))(linker)?;
        linker
            .func_wrap(
                edgion_wasm::host::MODULE,
                edgion_wasm::host::PROXY_HTTP_CALL,
                move |mut caller: wasmtime::Caller<'_, HostState>,
                      _cluster_ptr: i32,
                      _cluster_len: i32,
                      _headers_ptr: i32,
                      _headers_len: i32,
                      _body_ptr: i32,
                      _body_len: i32,
                      _trailers_ptr: i32,
                      _trailers_len: i32,
                      _timeout_ms: i32,
                      _token_ptr: i32|
                      -> i32 {
                    dispatch_calls.fetch_add(1, Ordering::SeqCst);
                    caller.data_mut().frame.pending_callouts.push(host_abi::PendingCallout {
                        token: 7,
                        origin: "http://example.invalid:80".to_string(),
                        path: "/".to_string(),
                        method: "bad method".to_string(),
                        authority: None,
                        headers: Vec::new(),
                        body: Vec::new(),
                        timeout_ms: 1,
                    });
                    host_abi::STATUS_OK
                },
            )
            .map(|_| ())
            .map_err(|e| format!("test callout linker setup: {e}"))
    }
}

/// Tick linker whose dispatch queues one callout against `origin`. Paired
/// with a listener that never answers, it parks the tick round inside the
/// callout response await for the full timeout.
fn blocking_tick_callout_linker(
    origin: String,
    dispatch_calls: Arc<AtomicUsize>,
) -> impl FnOnce(&mut Linker<HostState>) -> Result<(), String> + Send {
    move |linker| {
        panicking_linker(Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)), None)(linker)?;
        linker
            .func_wrap(
                edgion_wasm::host::MODULE,
                edgion_wasm::host::PROXY_HTTP_CALL,
                move |mut caller: wasmtime::Caller<'_, HostState>,
                      _cluster_ptr: i32,
                      _cluster_len: i32,
                      _headers_ptr: i32,
                      _headers_len: i32,
                      _body_ptr: i32,
                      _body_len: i32,
                      _trailers_ptr: i32,
                      _trailers_len: i32,
                      _timeout_ms: i32,
                      _token_ptr: i32|
                      -> i32 {
                    dispatch_calls.fetch_add(1, Ordering::SeqCst);
                    caller.data_mut().frame.pending_callouts.push(host_abi::PendingCallout {
                        token: 7,
                        origin: origin.clone(),
                        path: "/".to_string(),
                        method: "GET".to_string(),
                        authority: None,
                        headers: Vec::new(),
                        body: Vec::new(),
                        timeout_ms: BLOCKED_CALLOUT_TIMEOUT_MS,
                    });
                    host_abi::STATUS_OK
                },
            )
            .map(|_| ())
            .map_err(|e| format!("test callout linker setup: {e}"))
    }
}

/// Long enough that a cancelled round is unmistakably distinguishable from
/// one that simply ran to completion.
const BLOCKED_CALLOUT_TIMEOUT_MS: u32 = 30_000;

/// Poll the module's live tick-task count (maintained by
/// `TestTickTaskGuard`) until it reaches `expected`.
async fn wait_for_active_tick_tasks(state: &TickCalloutState, expected: usize, context: &str) {
    let wait = tokio::time::timeout(Duration::from_secs(5), async {
        while state.active_tick_tasks.load(Ordering::Acquire) != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(wait.is_ok(), "timed out waiting for {expected} tick task(s): {context}");
}

async fn wait_for_test_calls(calls: &AtomicUsize, minimum: usize) {
    for _ in 0..40 {
        if calls.load(Ordering::SeqCst) >= minimum {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {minimum} host calls");
}

/// Shared data is visible across leases (and thus across pooled VMs) of
/// the same module entry.
#[tokio::test]
async fn shared_data_guest_roundtrip_and_cross_lease_visibility() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "shared_data_guest_roundtrip_and_cross_lease_visibility",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "shared_data_guest_roundtrip_and_cross_lease_visibility",
        || async {
            let f = write_temp_wasm(SHARED_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let entry = load(&mgr, &file_config(f.path())).await.expect("loads");
            let mut lease = entry.acquire(100).expect("lease");
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            let size = lease.call_on_request_headers(0).expect("ok");
            assert_eq!(size, 5, "guest must read back the 5-byte value it wrote");
            drop(lease);
            // Host-side view (same store the guest wrote through the ABI):
            let (v, cas) = entry.shared_data_for_test().get("k").expect("key visible to host");
            assert_eq!(v, b"hello");
            assert!(cas >= 1);
        },
    )
    .await;
}

#[tokio::test]
async fn tick_host_panic_discards_vm_without_adopting_failed_state() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "tick_host_panic_discards_vm_without_adopting_failed_state",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "tick_host_panic_discards_vm_without_adopting_failed_state",
        || async {
            let file = write_temp_wasm(&panic_guest(PanicSite::Tick));
            let config = file_config(file.path());
            let manager = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
            let should_panic = Arc::new(AtomicBool::new(false));
            let calls = Arc::new(AtomicUsize::new(0));
            let entry = load_with_panicking_linker(&runtime, should_panic.clone(), calls.clone(), Some(0))
                .await
                .expect("tick panic guest loads");

            should_panic.store(true, Ordering::SeqCst);
            entry.activate_tick(MIN_TICK_PERIOD_MS);
            wait_for_test_calls(&calls, 2).await;
            entry.stop_tick();
            wait_for_active_tick_tasks(&entry.tick_callouts, 0, "tick stopped").await;

            assert_eq!(
                entry.tick_period_ms.load(Ordering::Acquire),
                MIN_TICK_PERIOD_MS,
                "failed on_tick state must not replace the last trusted period"
            );
            assert_eq!(
                entry.pool_len(),
                1,
                "timer failures must leave only the warm request VM"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn tick_callout_callback_panic_discards_vm_without_adopting_failed_state() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "tick_callout_callback_panic_discards_vm_without_adopting_failed_state",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "tick_callout_callback_panic_discards_vm_without_adopting_failed_state",
        || async {
            let file = write_temp_wasm(TICK_CALLOUT_PANIC_GUEST);
            let config = file_config(file.path());
            let manager = WasmVmManager::new_for_test();
            let should_panic = Arc::new(AtomicBool::new(false));
            let callback_calls = Arc::new(AtomicUsize::new(0));
            let dispatch_calls = Arc::new(AtomicUsize::new(0));
            let runtime = actual_runtime(&manager, &config, grant(vec!["http://example.invalid".to_string()], 10));
            let entry = runtime
                .get_or_load_with_linker(panicking_tick_callout_linker(
                    should_panic.clone(),
                    callback_calls.clone(),
                    dispatch_calls.clone(),
                ))
                .await
                .expect("tick callout panic guest loads");

            should_panic.store(true, Ordering::SeqCst);
            entry.activate_tick(MIN_TICK_PERIOD_MS);
            wait_for_test_calls(&callback_calls, 2).await;
            entry.stop_tick();
            wait_for_active_tick_tasks(&entry.tick_callouts, 0, "tick stopped").await;

            assert!(dispatch_calls.load(Ordering::SeqCst) >= 2);
            assert_eq!(
                entry.tick_period_ms.load(Ordering::Acquire),
                MIN_TICK_PERIOD_MS,
                "failed callout callback state must not replace the last trusted period"
            );
            assert_eq!(
                entry.pool_len(),
                1,
                "timer callback failures must leave only the warm request VM"
            );
        },
    )
    .await;
}

/// Plugin destruction must stop a tick round that is parked inside
/// a callout. The round holds the strong `Arc` that `Drop`'s abort depends
/// on, so without an explicit stop the entry (and its shared data) would
/// survive until the callout's own timeout expires.
#[tokio::test]
async fn plugin_drop_stops_a_tick_round_blocked_in_a_callout() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "plugin_drop_stops_a_tick_round_blocked_in_a_callout",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "plugin_drop_stops_a_tick_round_blocked_in_a_callout",
        || async {
            // Accept connections and never answer them: the callout stays in its
            // response await for the whole `BLOCKED_CALLOUT_TIMEOUT_MS` budget.
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let origin = format!("http://{}", listener.local_addr().expect("listener addr"));
            let connected = Arc::new(AtomicUsize::new(0));
            let accepted = connected.clone();
            let accept_task = tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok((stream, _)) = listener.accept().await {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    held.push(stream);
                }
            });

            let file = write_temp_wasm(TICK_CALLOUT_PANIC_GUEST);
            let config = file_config(file.path());
            let mgr = WasmVmManager::new_for_test();

            let callout_grant = grant(vec![origin.clone()], u64::from(BLOCKED_CALLOUT_TIMEOUT_MS));
            let runtime = actual_runtime(&mgr, &config, callout_grant);
            let dispatch_calls = Arc::new(AtomicUsize::new(0));
            let entry = runtime
                .get_or_load_with_linker(blocking_tick_callout_linker(origin, dispatch_calls.clone()))
                .await
                .expect("tick callout guest loads");
            let tick_state = entry.tick_callouts.clone();
            entry.activate_tick(MIN_TICK_PERIOD_MS);
            // Only the runtime may pin the entry; a test-held Arc would mask the bug.
            drop(entry);

            wait_for_test_calls(&dispatch_calls, 1).await;
            wait_for_test_calls(&connected, 1).await;

            let state = runtime.state.clone();
            drop(runtime);
            wait_for_active_tick_tasks(
                &tick_state,
                0,
                "plugin drop must stop a tick round still inside its callout",
            )
            .await;
            assert!(
                state.slot.published.load_full().is_none(),
                "dropped plugin must clear publication"
            );

            accept_task.abort();
        },
    )
    .await;
}

/// on_tick fires periodically once the guest sets a period during
/// configure, and stops when the module entry is dropped.
#[tokio::test]
async fn tick_task_drives_guest_on_tick() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "tick_task_drives_guest_on_tick",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "tick_task_drives_guest_on_tick",
        || async {
            let f = write_temp_wasm(TICK_GUEST);
            let mgr = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&mgr, &file_config(f.path()), CalloutGrant::none());
            let entry = runtime.get_or_load().await.expect("loads");
            // The eager warm instance created at load time already ran
            // on_configure, so the tick task is armed. Wait a few periods.
            let mut cas_seen = 0u32;
            for _ in 0..30 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Some((_, cas)) = entry.shared_data_for_test().get("t") {
                    cas_seen = cas;
                    if cas >= 2 {
                        break;
                    }
                }
            }
            assert!(cas_seen >= 2, "tick must have fired repeatedly, cas={cas_seen}");
        },
    )
    .await;
}

/// Tick-callout guest: on_configure reads the plugin configuration (the
/// callout origin string) into memory at 256/260 and requests a 100ms
/// tick. Every proxy_on_tick dispatches GET /check to that origin with a
/// static serialized header map; a rejected dispatch (nonzero status —
/// unarmed or denied) sets shared key `df`. The callback sets shared
/// `cb` on success (headers present) or `cf` on a failed callout
/// (0 headers).
const TICK_CALLOUT_GUEST: &str = r#"
    (module
      (import "env" "proxy_set_tick_period_milliseconds"
        (func $set_tick (param i32) (result i32)))
      (import "env" "proxy_get_buffer_bytes"
        (func $get_buf (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_dispatch_http_call"
        (func $dispatch (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_set_shared_data"
        (func $set_sd (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 8192))
      (data (i32.const 0) "df")
      (data (i32.const 4) "cf")
      (data (i32.const 8) "cb")
      (data (i32.const 16) "x")
      (data (i32.const 32) "\03\00\00\00\07\00\00\00\03\00\00\00\05\00\00\00\06\00\00\00\0a\00\00\00\04\00\00\00:method\00GET\00:path\00/check\00:authority\00test\00")
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $p i32)
        global.get $heap
        local.set $p
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $p)
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32)
        (drop (call $get_buf (i32.const 7) (i32.const 0) (i32.const 65536)
                             (i32.const 256) (i32.const 260)))
        (drop (call $set_tick (i32.const 100)))
        (i32.const 1))
      (func (export "proxy_on_tick") (param i32)
        (if (i32.ne (call $dispatch (i32.load (i32.const 256)) (i32.load (i32.const 260))
                              (i32.const 32) (i32.const 69)
                              (i32.const 0) (i32.const 0) (i32.const 0) (i32.const 0)
                              (i32.const 1000) (i32.const 264)) (i32.const 0))
          (then (drop (call $set_sd (i32.const 0) (i32.const 2) (i32.const 16) (i32.const 1) (i32.const 0))))))
      (func (export "proxy_on_http_call_response") (param i32 i32 i32 i32 i32)
        (if (i32.gt_s (local.get 2) (i32.const 0))
          (then (drop (call $set_sd (i32.const 8) (i32.const 2) (i32.const 16) (i32.const 1) (i32.const 0))))
          (else (drop (call $set_sd (i32.const 4) (i32.const 2) (i32.const 16) (i32.const 1) (i32.const 0))))))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32) (i32.const 0))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

fn grant(origins: Vec<String>, timeout_ms: u64) -> CalloutGrant {
    let mut allowlist: Vec<String> = origins
        .iter()
        .map(|o| edgion_resources::plugin_configs::normalize_callout_origin(o).expect("valid origin"))
        .collect();
    allowlist.sort();
    CalloutGrant {
        allowlist: Arc::from(allowlist),
        timeout_ms,
    }
}

/// Poll the module's shared data until `key` appears (tick markers are
/// written by the guest from the background tick task).
async fn wait_shared_key(entry: &Arc<ModuleEntry>, key: &str) -> bool {
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if entry.shared_data_for_test().get(key).is_some() {
            return true;
        }
    }
    false
}

/// Tick-callout happy path: the plugin's non-empty allowlist arms
/// dispatch in the tick phase; the callout hits the target and the
/// success callback is delivered on the root context.
#[tokio::test]
async fn tick_callout_dispatches_and_delivers_response() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "tick_callout_dispatches_and_delivers_response",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "tick_callout_dispatches_and_delivers_response",
        || async {
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/check"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(b"ok".to_vec()))
                .mount(&server)
                .await;

            let f = write_temp_wasm(TICK_CALLOUT_GUEST);
            let mut config = file_config(f.path());
            config.plugin_config = Some(WasmRawConfig::string(server.uri()));
            let mgr = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&mgr, &config, grant(vec![server.uri()], 1000));

            let entry = runtime.get_or_load().await.expect("loads");
            assert!(
                wait_shared_key(&entry, "cb").await,
                "success callback must be delivered to the root context"
            );
            assert!(
                !server.received_requests().await.unwrap().is_empty(),
                "the tick callout must reach the target"
            );
        },
    )
    .await;
}

/// Unreachable target: dispatch is accepted (origin allowlisted) but the
/// callout fails, so the guest sees the 0-header failure callback.
#[tokio::test]
async fn tick_callout_unreachable_delivers_failure_callback() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "tick_callout_unreachable_delivers_failure_callback",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "tick_callout_unreachable_delivers_failure_callback",
        || async {
            let f = write_temp_wasm(TICK_CALLOUT_GUEST);
            let mut config = file_config(f.path());
            // Port 1 is unreachable; still allowlisted so dispatch succeeds.
            config.plugin_config = Some(WasmRawConfig::string("http://127.0.0.1:1"));
            let mgr = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&mgr, &config, grant(vec!["http://127.0.0.1:1".to_string()], 300));

            let entry = runtime.get_or_load().await.expect("loads");
            assert!(
                wait_shared_key(&entry, "cf").await,
                "failed callout must deliver the 0-header callback"
            );
        },
    )
    .await;
}

/// Target not in the allowlist: dispatch is rejected in the tick phase
/// exactly as in the request phase, and no request leaves the host.
#[tokio::test]
async fn tick_callout_denied_by_allowlist() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "tick_callout_denied_by_allowlist",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "tick_callout_denied_by_allowlist",
        || async {
            use wiremock::{Mock, MockServer, ResponseTemplate};

            let server = MockServer::start().await;
            Mock::given(wiremock::matchers::any())
                .respond_with(ResponseTemplate::new(200))
                .mount(&server)
                .await;

            let f = write_temp_wasm(TICK_CALLOUT_GUEST);
            let mut config = file_config(f.path());
            config.plugin_config = Some(WasmRawConfig::string(server.uri()));
            let mgr = WasmVmManager::new_for_test();
            // Dispatch IS armed, but for a different origin than the guest targets.
            let runtime = actual_runtime(
                &mgr,
                &config,
                grant(vec!["http://allowed.example.com".to_string()], 1000),
            );

            let entry = runtime.get_or_load().await.expect("loads");
            assert!(wait_shared_key(&entry, "df").await, "guest must observe the denial");
            assert!(
                server.received_requests().await.unwrap().is_empty(),
                "no callout may reach a non-allowlisted origin"
            );
        },
    )
    .await;
}

/// Fix 1 regression test: a failed engine init (e.g. pooling-allocator
/// reservation refused under a restrictive RLIMIT_AS) must degrade to a
/// normal load-style error on every call path, never panic. Forcing a
/// real `Engine::new` failure isn't portable in a test environment, so
/// this exercises the `new_with_engine_result` seam directly.
#[tokio::test]
async fn engine_init_failure_fails_closed_not_panic() {
    let mgr = WasmVmManager::new_with_engine_result(Err("wasm engine init failed: test".to_string()));
    let f = write_temp_wasm(MINI_GUEST);
    let config = file_config(f.path());

    let err = load(&mgr, &config).await.unwrap_err();
    assert!(err.contains("wasm engine init failed"), "got: {err}");

    // The module never gets far enough to populate the cache, so a
    // sync-path caller sees a plain miss rather than a panic too.
    assert!(actual_runtime(&mgr, &config, CalloutGrant::none()).try_get().is_none());
}

#[tokio::test]
async fn epoch_ticker_spawn_failure_uses_engine_unavailable_path() {
    let manager = Arc::new(WasmVmManager::new_with_epoch_ticker_spawner(|_| {
        Err(std::io::Error::other("forced ticker spawn failure"))
    }));
    assert!(manager.epoch_ticker.is_none());
    let engine_error = manager.engine().expect_err("ticker failure disables the engine");
    assert!(engine_error.starts_with("wasm epoch ticker init failed:"));

    let file = write_temp_wasm(MINI_GUEST);
    let config = file_config(file.path());
    let runtime = actual_runtime(&manager, &config, CalloutGrant::none());
    assert!(runtime.get_or_load().await.unwrap_err().contains("epoch ticker"));
    runtime.spawn_preload();
    assert!(runtime.try_get().is_none());
    assert!(!runtime.state.slot.preload_in_flight.load(Ordering::Acquire));
}

#[tokio::test]
async fn oci_source_loads_and_always_recheck_swaps_on_digest_change() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "oci_source_loads_and_always_recheck_swaps_on_digest_change",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "oci_source_loads_and_always_recheck_swaps_on_digest_change",
        || async {
            use sha2::{Digest as _, Sha256};
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            const WASM_A: &[u8] = b"\0asm\x01\x00\x00\x00";
            // Same module plus an empty custom section: different bytes, still valid.
            const WASM_B: &[u8] = b"\0asm\x01\x00\x00\x00\x00\x01\x00";

            fn digest_of(bytes: &[u8]) -> String {
                format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
            }
            fn manifest_for(blob: &[u8]) -> Vec<u8> {
                serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 2,
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "layers": [{
                        "mediaType": "application/vnd.module.wasm.content.layer.v1+wasm",
                        "digest": digest_of(blob),
                        "size": blob.len()
                    }]
                }))
                .unwrap()
            }
            async fn mount(server: &MockServer, blob: &'static [u8]) {
                Mock::given(method("GET"))
                    .and(path("/v2/test/wasm/manifests/latest"))
                    .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest_for(blob)))
                    .mount(server)
                    .await;
                Mock::given(method("GET"))
                    .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(blob))))
                    .respond_with(ResponseTemplate::new(200).set_body_bytes(blob.to_vec()))
                    .mount(server)
                    .await;
            }

            let server = MockServer::start().await;
            mount(&server, WASM_A).await;
            let host = server.uri().trim_start_matches("http://").to_string();
            let config: WasmConfig = serde_json::from_value(serde_json::json!({
                // tag `latest` => effective pullPolicy Always
                "maxActiveRequests": 1,
                "source": { "oci": { "image": format!("{host}/test/wasm:latest") } },
                "timeoutMs": 500
            }))
            .unwrap();

            let mgr = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&mgr, &config, CalloutGrant::none());
            let entry1 = runtime.get_or_load().await.unwrap();
            // Within the re-check interval: cached entry, no new resolution.
            let entry1b = runtime.get_or_load().await.unwrap();
            assert!(Arc::ptr_eq(&entry1, &entry1b));

            let pinned = entry1.acquire(100).unwrap();
            // Registry moves the tag to new content; backdate to force a re-check.
            server.reset().await;
            mount(&server, WASM_B).await;
            runtime.backdate_oci_check_for_test().await;
            let during_refresh = runtime.get_or_load().await.unwrap();
            assert!(
                Arc::ptr_eq(&entry1, &during_refresh),
                "the last-known-good entry must remain published while refresh runs"
            );
            runtime.wait_for_oci_refresh_for_test().await;
            let entry2 = runtime.try_get().expect("refreshed entry is published");
            assert!(!Arc::ptr_eq(&entry1, &entry2), "digest change must reload the module");
            assert!(entry2.acquire(100).is_err(), "old generation retains runtime admission");
            drop(pinned);
            drop(entry2.acquire(100).unwrap());

            // Unchanged digest after another backdate: entry is kept.
            runtime.backdate_oci_check_for_test().await;
            let entry2b = runtime.get_or_load().await.unwrap();
            assert!(Arc::ptr_eq(&entry2, &entry2b));
            runtime.wait_for_oci_refresh_for_test().await;
            assert!(Arc::ptr_eq(
                &entry2,
                &runtime.try_get().expect("unchanged entry stays published")
            ));

            // A moved tag whose bytes fail compilation must not replace the
            // healthy entry or turn the slot into the cold-load Failed state.
            const INVALID_WASM: &[u8] = b"not a wasm module";
            server.reset().await;
            mount(&server, INVALID_WASM).await;
            runtime.backdate_oci_check_for_test().await;
            assert!(Arc::ptr_eq(&entry2, &runtime.get_or_load().await.unwrap()));
            runtime.wait_for_oci_refresh_for_test().await;
            assert!(Arc::ptr_eq(
                &entry2,
                &runtime.try_get().expect("compile failure preserves LKG")
            ));

            // A policy-only reload gets a distinct slot. The old Always task may
            // finish after the IfNotPresent slot is published, but cannot commit
            // into or replace that new-policy slot.
            server.reset().await;
            Mock::given(method("GET"))
                .and(path("/v2/test/wasm/manifests/latest"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_delay(Duration::from_millis(250))
                        .set_body_bytes(manifest_for(WASM_A)),
                )
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/v2/test/wasm/blobs/{}", digest_of(WASM_A))))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM_A.to_vec()))
                .mount(&server)
                .await;
            runtime.backdate_oci_check_for_test().await;
            assert!(Arc::ptr_eq(&entry2, &runtime.get_or_load().await.unwrap()));
            let mut if_not_present = config.clone();
            if_not_present.source.oci.as_mut().unwrap().pull_policy =
                Some(edgion_resources::plugin_configs::OciPullPolicy::IfNotPresent);
            let new_runtime = actual_runtime(&mgr, &if_not_present, CalloutGrant::none());
            let if_not_present_entry = new_runtime
                .get_or_load()
                .await
                .expect("new-policy slot loads independently");
            runtime.wait_for_oci_refresh_for_test().await;
            assert!(
                Arc::ptr_eq(
                    &if_not_present_entry,
                    &new_runtime.try_get().expect("new-policy entry remains published")
                ),
                "the old Always task must not publish into the new-policy slot"
            );
        },
    )
    .await;
}

#[tokio::test]
async fn oci_always_refresh_is_non_blocking_single_flight_and_keeps_lkg_on_failure() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "oci_always_refresh_is_non_blocking_single_flight_and_keeps_lkg_on_failure",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "oci_always_refresh_is_non_blocking_single_flight_and_keeps_lkg_on_failure",
        || async {
            use sha2::{Digest as _, Sha256};
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            const WASM: &[u8] = b"\0asm\x01\x00\x00\x00";
            let digest = format!("sha256:{}", hex::encode(Sha256::digest(WASM)));
            let manifest = serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "layers": [{
                    "mediaType": "application/vnd.module.wasm.content.layer.v1+wasm",
                    "digest": digest,
                    "size": WASM.len()
                }]
            }))
            .unwrap();
            let manifest_path = "/v2/test/wasm/manifests/latest";
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path(manifest_path))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest.clone()))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/v2/test/wasm/blobs/{digest}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
                .mount(&server)
                .await;

            let host = server.uri().trim_start_matches("http://").to_string();
            let config: WasmConfig = serde_json::from_value(serde_json::json!({
                "source": { "oci": { "image": format!("{host}/test/wasm:latest") } },
                "timeoutMs": 500
            }))
            .unwrap();
            let mgr = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&mgr, &config, CalloutGrant::none());
            let lkg = runtime.get_or_load().await.unwrap();

            server.reset().await;
            Mock::given(method("GET"))
                .and(path(manifest_path))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_delay(Duration::from_millis(250))
                        .set_body_bytes(manifest),
                )
                .mount(&server)
                .await;
            runtime.backdate_oci_check_for_test().await;

            let request_entry = tokio::time::timeout(Duration::from_millis(50), runtime.get_or_load())
                .await
                .expect("request lookup must not wait for the delayed manifest")
                .unwrap();
            assert!(Arc::ptr_eq(&lkg, &request_entry));
            for _ in 0..32 {
                let response_entry = runtime
                    .try_get_or_schedule_refresh()
                    .expect("response-only lookup serves LKG");
                assert!(Arc::ptr_eq(&lkg, &response_entry));
            }
            runtime.wait_for_oci_refresh_for_test().await;
            assert_eq!(
                server.received_requests().await.unwrap().len(),
                1,
                "concurrent request/response claims must share one manifest re-check"
            );
            assert!(Arc::ptr_eq(
                &lkg,
                &runtime.try_get().expect("unchanged LKG remains published")
            ));

            // A definitive registry failure advances the refresh deadline but
            // cannot clear or replace the healthy published module.
            server.reset().await;
            Mock::given(method("GET"))
                .and(path(manifest_path))
                .respond_with(ResponseTemplate::new(400))
                .mount(&server)
                .await;
            runtime.backdate_oci_check_for_test().await;
            let response_entry = runtime
                .try_get_or_schedule_refresh()
                .expect("failed refresh still serves LKG");
            assert!(Arc::ptr_eq(&lkg, &response_entry));
            runtime.wait_for_oci_refresh_for_test().await;
            assert!(Arc::ptr_eq(
                &lkg,
                &runtime.try_get().expect("failed refresh preserves LKG")
            ));
        },
    )
    .await;
}

#[tokio::test]
async fn oci_refresh_stops_old_tick_and_starts_new_tick() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "oci_refresh_stops_old_tick_and_starts_new_tick",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "oci_refresh_stops_old_tick_and_starts_new_tick",
        || async {
            use sha2::{Digest as _, Sha256};
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            fn digest_of(bytes: &[u8]) -> String {
                format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
            }
            fn manifest_for(blob: &[u8]) -> Vec<u8> {
                serde_json::to_vec(&serde_json::json!({
                    "schemaVersion": 2,
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "layers": [{
                        "mediaType": "application/vnd.module.wasm.content.layer.v1+wasm",
                        "digest": digest_of(blob),
                        "size": blob.len()
                    }]
                }))
                .unwrap()
            }
            async fn mount(server: &MockServer, blob: Vec<u8>) {
                let digest = digest_of(&blob);
                Mock::given(method("GET"))
                    .and(path("/v2/test/tick/manifests/latest"))
                    .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest_for(&blob)))
                    .mount(server)
                    .await;
                Mock::given(method("GET"))
                    .and(path(format!("/v2/test/tick/blobs/{digest}")))
                    .respond_with(ResponseTemplate::new(200).set_body_bytes(blob))
                    .mount(server)
                    .await;
            }
            let wasm_a = wat::parse_str(TICK_GUEST).unwrap();
            let wasm_b = wat::parse_str(TICK_GUEST.replace(
                "(func (export \"proxy_on_tick\")",
                "(func (export \"version_b\"))\n      (func (export \"proxy_on_tick\")",
            ))
            .unwrap();
            let server = MockServer::start().await;
            mount(&server, wasm_a).await;
            let server_uri = server.uri();
            let host = server_uri.trim_start_matches("http://");
            let config: WasmConfig = serde_json::from_value(serde_json::json!({
                "source": { "oci": { "image": format!("{host}/test/tick:latest") } },
                "timeoutMs": 500
            }))
            .unwrap();
            let mgr = WasmVmManager::new_for_test();
            let runtime = actual_runtime(&mgr, &config, CalloutGrant::none());
            let old_entry = runtime.get_or_load().await.unwrap();
            let tick_state = old_entry.tick_callouts.clone();
            wait_for_active_tick_tasks(&tick_state, 1, "the published entry owns the tick task").await;

            server.reset().await;
            mount(&server, wasm_b).await;
            runtime.backdate_oci_check_for_test().await;
            let served_during_refresh = runtime.get_or_load().await.unwrap();
            assert!(Arc::ptr_eq(&old_entry, &served_during_refresh));
            runtime.wait_for_oci_refresh_for_test().await;
            let new_entry = runtime.try_get().expect("new entry published");
            assert!(!Arc::ptr_eq(&old_entry, &new_entry));
            wait_for_active_tick_tasks(&tick_state, 1, "the refreshed entry owns the tick task").await;

            assert!(old_entry.tick_task.lock().is_none(), "old tick task must be stopped");
            assert!(!old_entry.tick_enabled.load(Ordering::Acquire));
            assert!(new_entry.tick_enabled.load(Ordering::Acquire));

            // Holding the old Arc models an in-flight request. Tick shutdown must
            // not poison or otherwise prevent its ordinary hooks from completing.
            let mut old_lease = old_entry.acquire(500).expect("old request lease remains usable");
            old_lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            assert_eq!(old_lease.call_on_request_headers(0).unwrap(), 0);
        },
    )
    .await;
}

#[tokio::test]
async fn oci_source_verifies_top_level_sha256_against_final_bytes() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "oci_source_verifies_top_level_sha256_against_final_bytes",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "oci_source_verifies_top_level_sha256_against_final_bytes",
        || async {
            use sha2::{Digest as _, Sha256};
            use wiremock::matchers::{method, path};
            use wiremock::{Mock, MockServer, ResponseTemplate};

            const WASM: &[u8] = b"\0asm\x01\x00\x00\x00";
            let digest = format!("sha256:{}", hex::encode(Sha256::digest(WASM)));
            let manifest = serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "layers": [{
                    "mediaType": "application/vnd.module.wasm.content.layer.v1+wasm",
                    "digest": digest,
                    "size": WASM.len()
                }]
            }))
            .unwrap();
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v2/test/wasm/manifests/1.0"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(manifest))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/v2/test/wasm/blobs/{digest}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(WASM.to_vec()))
                .mount(&server)
                .await;

            let host = server.uri().trim_start_matches("http://").to_string();
            let config: WasmConfig = serde_json::from_value(serde_json::json!({
                "source": { "oci": { "image": format!("{host}/test/wasm:1.0") } },
                "sha256": "0".repeat(64),
                "timeoutMs": 500
            }))
            .unwrap();
            let mgr = WasmVmManager::new_for_test();
            let err = actual_runtime(&mgr, &config, CalloutGrant::none())
                .get_or_load()
                .await
                .unwrap_err();
            assert!(err.contains("sha256 mismatch"), "got: {err}");
        },
    )
    .await;
}

fn wasm_owner(config: &WasmConfig) -> edgion_resources::EdgionPlugins {
    serde_json::from_value(serde_json::json!({
        "metadata": {"name": "wasm-test", "namespace": "default"},
        "spec": {"requestPlugins": [{"type": "Wasm", "config": config}]}
    }))
    .unwrap()
}

pub(in crate::plugins::http::wasm) fn actual_runtime(
    manager: &WasmVmManager,
    config: &WasmConfig,
    grant: CalloutGrant,
) -> WasmRuntime {
    let owner = wasm_owner(config);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    manager
        .new_runtime(
            &entry,
            &edgion_common::link_sys_core::outbound_tls::outbound_global_default(),
            grant,
        )
        .unwrap()
}
fn actual_url_client(
    url: &str,
    fetch: Option<&edgion_resources::plugin_configs::WasmFetchSpec>,
) -> Result<reqwest::Client, String> {
    let mut config = WasmConfig::default();
    config.source.url = Some(url.to_string());
    config.source.fetch = fetch.cloned();
    let owner = wasm_owner(&config);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let defaults = edgion_common::link_sys_core::outbound_tls::outbound_global_default();
    let proof = super::super::plugin::WasmPlugin::validate_tls_entry(&entry, &defaults)
        .unwrap()
        .unwrap();
    prepare_url_client(fetch, &proof, &defaults)
}

pub(crate) fn wasm_tls_owner() -> (edgion_resources::EdgionPlugins, Vec<u8>, Vec<u8>) {
    let generated = rcgen::generate_simple_self_signed(vec!["artifact.example.com".into()]).unwrap();
    let pem = generated.cert.pem().into_bytes();
    let key = generated.signing_key.serialize_pem().into_bytes();
    let secret = |name: &str, values: &[(&str, &[u8])]| k8s_openapi::api::core::v1::Secret {
        metadata: kube::core::ObjectMeta {
            name: Some(name.into()),
            namespace: Some("default".into()),
            resource_version: Some("7".into()),
            ..Default::default()
        },
        data: Some(
            values
                .iter()
                .map(|(name, bytes)| ((*name).into(), k8s_openapi::ByteString(bytes.to_vec())))
                .collect(),
        ),
        ..Default::default()
    };
    let owner = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"wasm-test","uid":"owner"},
        "spec":{"requestPlugins":[{"type":"Wasm","config":{"sha256":"0".repeat(64),"source":{
            "url":"https://artifact.example.com/module.wasm","fetch":{"tls":{
                "enabled":true,"verify":true,
                "validation":{"caCertificateRefs":[{"kind":"Secret","group":"core","name":"ca"}]},
                "clientCertificateRef":{"group":"core","name":"client"},
                "resolvedCaCertificates":[secret("ca", &[("ca.crt", &pem)])],
                "resolvedClientCertificate":secret("client", &[("tls.crt", &pem),("tls.key", &key)])
            }}}
        }}]}
    }))
    .unwrap();
    (owner, pem, key)
}

#[test]
fn wasm_url_material_is_bound_before_private_runtime_creation() {
    use edgion_common::link_sys_core::outbound_tls::{
        resolve_validated_outbound_tls, test_util::snapshot, EffectiveCa,
    };
    use edgion_resources::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    let defaults = snapshot(Default::default());
    let manager = WasmVmManager::new_for_test();
    let (owner, pem, key) = wasm_tls_owner();
    for fail_open in [false, true] {
        for case in [
            "matching",
            "configmap",
            "name",
            "missing-name",
            "ns",
            "missing-ns",
            "empty-ns",
            "client-name",
            "client-missing-name",
            "client-ns",
            "client-missing-ns",
            "client-empty-ns",
            "provenance",
            "client-provenance",
            "retarget",
            "no-ref",
            "no-material",
            "no-client",
            "owner-name",
            "owner-ns",
        ] {
            let mut candidate = serde_json::to_value(&owner).unwrap();
            let config = &mut candidate["spec"]["requestPlugins"][0]["config"];
            config["failOpen"] = serde_json::json!(fail_open);
            let tls = &mut config["source"]["fetch"]["tls"];
            match case {
                "configmap" => {
                    tls["validation"]["caCertificateRefs"][0]["kind"] = serde_json::json!("ConfigMap");
                    tls["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                        serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"});
                }
                "name" => tls["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::json!("wrong"),
                "missing-name" => tls["resolvedCaCertificates"][0]["metadata"]["name"] = serde_json::Value::Null,
                "ns" => tls["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::json!("other"),
                "missing-ns" => tls["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::Value::Null,
                "empty-ns" => tls["resolvedCaCertificates"][0]["metadata"]["namespace"] = serde_json::json!(""),
                "client-name" => tls["resolvedClientCertificate"]["metadata"]["name"] = serde_json::json!("wrong"),
                "client-missing-name" => tls["resolvedClientCertificate"]["metadata"]["name"] = serde_json::Value::Null,
                "client-ns" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::json!("other"),
                "client-missing-ns" => {
                    tls["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::Value::Null
                }
                "client-empty-ns" => tls["resolvedClientCertificate"]["metadata"]["namespace"] = serde_json::json!(""),
                "provenance" => {
                    tls["resolvedCaCertificates"][0]["metadata"]["annotations"] =
                        serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"})
                }
                "client-provenance" => {
                    tls["resolvedClientCertificate"]["metadata"]["annotations"] =
                        serde_json::json!({(CONTROLLER_CA_SOURCE):"ConfigMap"})
                }
                "retarget" => tls["validation"]["caCertificateRefs"][0]["name"] = serde_json::json!("next"),
                "no-ref" => tls["validation"]["caCertificateRefs"] = serde_json::Value::Null,
                "no-material" => tls["resolvedCaCertificates"] = serde_json::Value::Null,
                "no-client" => tls["resolvedClientCertificate"] = serde_json::Value::Null,
                "owner-name" => candidate["metadata"]["name"] = serde_json::Value::Null,
                "owner-ns" => candidate["metadata"]["namespace"] = serde_json::json!(""),
                _ => {}
            }
            let candidate: edgion_resources::EdgionPlugins = serde_json::from_value(candidate).unwrap();
            let prepared = PreparedPluginReferences::from_plugins(&candidate);
            let binding = prepared.bind(&candidate).unwrap();
            let entry = binding.request_entries().next().unwrap();
            if matches!(case, "matching" | "configmap") {
                let proof = super::super::plugin::WasmPlugin::validate_tls_entry(&entry, &defaults)
                    .unwrap()
                    .unwrap();
                let resolved = resolve_validated_outbound_tls(&proof, true, &defaults).unwrap();
                let EffectiveCa::Custom(bundles) = resolved.ca else {
                    panic!("expected bound CA")
                };
                assert_eq!(bundles, vec![pem.clone()]);
                let (cert, actual_key) = resolved.client_identity.unwrap();
                assert_eq!(cert, pem);
                assert_eq!(actual_key.as_slice(), key.as_slice());
                let runtime = manager.new_runtime(&entry, &defaults, CalloutGrant::none()).unwrap();
                assert!(runtime.state.url_client.as_ref().unwrap().is_ok());
                assert!(runtime.config().source.fetch.as_ref().unwrap().tls.is_none());
                let EdgionPlugin::Wasm(authored) = entry.config() else {
                    panic!()
                };
                assert!(authored.source.fetch.as_ref().unwrap().tls.is_some());
            } else {
                assert_eq!(
                    manager
                        .new_runtime(&entry, &defaults, CalloutGrant::none())
                        .err()
                        .as_deref(),
                    Some("wasm TLS configuration is invalid"),
                    "{case}"
                );
            }
        }
    }
}

#[tokio::test]
async fn wasm_valid_identity_crypto_failure_remains_negative_cached_load_error() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "wasm_valid_identity_crypto_failure_remains_negative_cached_load_error",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "wasm_valid_identity_crypto_failure_remains_negative_cached_load_error",
        || async {
            let (mut owner, _, _) = wasm_tls_owner();
            let EdgionPlugin::Wasm(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                panic!()
            };
            config
                .source
                .fetch
                .as_mut()
                .unwrap()
                .tls
                .as_mut()
                .unwrap()
                .resolved_ca_certificates
                .as_mut()
                .unwrap()[0]
                .data
                .as_mut()
                .unwrap()
                .insert("ca.crt".into(), k8s_openapi::ByteString(b"invalid PEM".to_vec()));
            let defaults = edgion_common::link_sys_core::outbound_tls::test_util::snapshot(Default::default());
            let prepared = PreparedPluginReferences::from_plugins(&owner);
            let binding = prepared.bind(&owner).unwrap();
            let entry = binding.request_entries().next().unwrap();
            let manager = WasmVmManager::new_for_test();
            let runtime = manager.new_runtime(&entry, &defaults, CalloutGrant::none()).unwrap();
            assert!(runtime.state.url_client.as_ref().unwrap().is_err());
            let first = runtime.get_or_load().await.unwrap_err();
            assert!(first.starts_with("wasm fetch client:"), "{first}");
            assert_eq!(runtime.get_or_load().await.unwrap_err(), first);
            assert!(runtime.try_get().is_none());
            assert!(runtime.state.prepare_preload().is_none());
        },
    )
    .await;
}

#[test]
fn wasm_captured_globals_and_disabled_https_are_process_isolated() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::wasm::vm_manager::tests",
        "wasm_captured_globals_and_disabled_https_are_process_isolated",
        "EDGION_WASM_CAPTURED_GLOBAL_CHILD",
        "captured",
        || {
            use edgion_common::link_sys_core::outbound_tls::{
                outbound_global_default, resolve_validated_outbound_tls, set_outbound_global_default,
                test_util::snapshot, EffectiveCa,
            };
            use edgion_resources::resources::common::OutboundTlsConfig;
            let previous = outbound_global_default();
            let (mut owner, pem, key) = wasm_tls_owner();
            let EdgionPlugin::Wasm(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                panic!()
            };
            let tls = config.source.fetch.as_mut().unwrap().tls.as_mut().unwrap();
            let mut global = OutboundTlsConfig {
                verify: true,
                validation: tls.outbound.validation.take(),
                client_certificate_ref: tls.outbound.client_certificate_ref.take(),
                resolved_ca_certificates: tls.resolved_ca_certificates.take(),
                resolved_client_certificate: tls.resolved_client_certificate.take(),
            };
            global.client_certificate_ref.as_mut().unwrap().namespace = Some("default".into());
            global
                .validation
                .as_mut()
                .unwrap()
                .ca_certificate_refs
                .as_mut()
                .unwrap()[0]
                .namespace = Some("default".into());
            let captured = snapshot(global.clone());
            global.resolved_client_certificate.as_mut().unwrap().metadata.name = Some("wrong".into());
            let invalid = snapshot(global);
            set_outbound_global_default(invalid.clone());
            let manager = WasmVmManager::new_for_test();
            let runtime = {
                let prepared = PreparedPluginReferences::from_plugins(&owner);
                let binding = prepared.bind(&owner).unwrap();
                let entry = binding.request_entries().next().unwrap();
                let proof = super::super::plugin::WasmPlugin::validate_tls_entry(&entry, &captured)
                    .unwrap()
                    .unwrap();
                let resolved = resolve_validated_outbound_tls(&proof, true, &captured).unwrap();
                let EffectiveCa::Custom(bundles) = resolved.ca else {
                    panic!("captured CA")
                };
                assert_eq!(bundles, vec![pem.clone()]);
                let (cert, actual_key) = resolved.client_identity.unwrap();
                assert_eq!(cert, pem);
                assert_eq!(actual_key.as_slice(), key.as_slice());
                let runtime = manager.new_runtime(&entry, &captured, CalloutGrant::none()).unwrap();
                set_outbound_global_default(captured.clone());
                assert!(manager.new_runtime(&entry, &invalid, CalloutGrant::none()).is_err());
                runtime
            };
            drop(owner);
            drop(captured);
            assert!(runtime.state.url_client.as_ref().unwrap().is_ok());
            assert_eq!(
                runtime.config().source.url.as_deref(),
                Some("https://artifact.example.com/module.wasm")
            );
            let (mut local, _, _) = wasm_tls_owner();
            {
                let prepared = PreparedPluginReferences::from_plugins(&local);
                let binding = prepared.bind(&local).unwrap();
                assert!(manager
                    .new_runtime(
                        &binding.request_entries().next().unwrap(),
                        &invalid,
                        CalloutGrant::none()
                    )
                    .is_ok());
            }
            let EdgionPlugin::Wasm(config) = &mut local.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                panic!()
            };
            let tls = config.source.fetch.as_mut().unwrap().tls.as_mut().unwrap();
            tls.enabled = false;
            tls.outbound.verify = Some(false);
            tls.resolved_ca_certificates.as_mut().unwrap()[0].metadata.name = Some("wrong".into());
            tls.resolved_client_certificate.as_mut().unwrap().metadata.name = Some("wrong".into());
            {
                let prepared = PreparedPluginReferences::from_plugins(&local);
                let binding = prepared.bind(&local).unwrap();
                let entry = binding.request_entries().next().unwrap();
                let proof = super::super::plugin::WasmPlugin::validate_tls_entry(&entry, &invalid)
                    .unwrap()
                    .unwrap();
                assert!(proof.active_field().is_none());
                // The canonical HTTP factory skips TLS resolution for inactive
                // selection; a direct low-level resolver call would inherit globals.
                let runtime = manager.new_runtime(&entry, &invalid, CalloutGrant::none()).unwrap();
                assert!(runtime.state.url_client.as_ref().unwrap().is_ok());
                assert!(runtime.config().source.url.as_ref().unwrap().starts_with("https://"));
            }
            let mut absent = local.clone();
            let EdgionPlugin::Wasm(config) = &mut absent.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                panic!()
            };
            config.source.fetch.as_mut().unwrap().tls = None;
            {
                let prepared = PreparedPluginReferences::from_plugins(&absent);
                let binding = prepared.bind(&absent).unwrap();
                let entry = binding.request_entries().next().unwrap();
                let runtime = manager.new_runtime(&entry, &invalid, CalloutGrant::none()).unwrap();
                assert!(runtime.state.url_client.as_ref().unwrap().is_ok());
                assert!(runtime.config().source.url.as_ref().unwrap().starts_with("https://"));
            }
            let EdgionPlugin::Wasm(config) = &mut local.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                panic!()
            };
            config.source.url = None;
            config.source.file = Some("inactive.wasm".into());
            config.source.fetch.as_mut().unwrap().tls.as_mut().unwrap().enabled = true;
            let prepared = PreparedPluginReferences::from_plugins(&local);
            let binding = prepared.bind(&local).unwrap();
            let entry = binding.request_entries().next().unwrap();
            assert!(super::super::plugin::WasmPlugin::validate_tls_entry(&entry, &invalid)
                .unwrap()
                .is_none());
            let inactive = manager.new_runtime(&entry, &invalid, CalloutGrant::none()).unwrap();
            assert!(inactive.state.url_client.is_none());
            assert!(inactive.config().source.fetch.as_ref().unwrap().tls.is_some());
            set_outbound_global_default(previous);
        },
    );
}

#[test]
fn actual_instance_admission_includes_idle_and_releases_failed_initialization() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::wasm::vm_manager::tests",
        "actual_instance_admission_includes_idle_and_releases_failed_initialization",
        "EDGION_WASM_INSTANCE_BOUND_CHILD",
        "bounded",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let permits = instance_permits().clone();
                let reserved = permits
                    .clone()
                    .try_acquire_many_owned((super::super::admission::limits().max_instances - 1) as u32)
                    .unwrap();
                let manager = WasmVmManager::new_for_test();
                let file = write_temp_wasm(MINI_GUEST);
                let entry = load(&manager, &file_config(file.path())).await.unwrap();
                assert_eq!(permits.available_permits(), 0, "warm idle VM counts");
                let lease = entry.acquire(100).unwrap();
                assert!(
                    entry.acquire(100).is_err(),
                    "fresh instance cannot bypass process admission"
                );
                assert!(entry.acquire_timer().is_err(), "dedicated timer counts too");
                drop(lease);
                assert_eq!(permits.available_permits(), 0);
                drop(entry);
                assert_eq!(permits.available_permits(), 1);
                let bad = write_temp_wasm(
                    r#"(module (memory (export "memory") 1)
                    (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 0)))"#,
                );
                assert!(load(&manager, &file_config(bad.path())).await.is_err());
                assert_eq!(
                    permits.available_permits(),
                    1,
                    "failed initialization releases admission"
                );
                drop(reserved);
                assert_eq!(
                    permits.available_permits(),
                    super::super::admission::limits().max_instances
                );
            });
        },
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pool_metric_aggregates_generations_and_old_retirement_does_not_resurrect() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "pool_metric_aggregates_generations_and_old_retirement_does_not_resurrect",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "pool_metric_aggregates_generations_and_old_retirement_does_not_resurrect",
        || async {
            use edgion_common::observe::{
                cardinality_guard::CardinalityMode,
                metrics_api::{RecorderConfig, TestRecorder},
            };
            let recorder = TestRecorder::with_clock(RecorderConfig {
                service: "pool-fixture",
                idle_timeout_secs: 1,
                histogram_buckets: &[],
                cardinality_cap: 10000,
                cardinality_mode: CardinalityMode::Enforce,
            })
            .unwrap();
            let file = write_temp_wasm(MINI_GUEST);
            let manager = WasmVmManager::new_for_test();
            let mut old_cfg = file_config(file.path());
            old_cfg.plugin_config = Some(WasmRawConfig::string("old"));
            let mut new_cfg = old_cfg.clone();
            new_cfg.plugin_config = Some(WasmRawConfig::string("replacement"));
            let old_runtime = actual_runtime(&manager, &old_cfg, CalloutGrant::none());
            let new_runtime = actual_runtime(&manager, &new_cfg, CalloutGrant::none());
            let (old, new) = recorder.with(|| {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        (
                            old_runtime.get_or_load().await.unwrap(),
                            new_runtime.get_or_load().await.unwrap(),
                        )
                    })
                })
            });
            assert!(!Arc::ptr_eq(&old, &new));
            assert_eq!(old.metrics_label, new.metrics_label);
            let label = old.metrics_label.clone();
            let assert_pool = |expected: usize| {
                let output = recorder.render();
                assert!(
                    output.lines().any(|line| line.starts_with("edgion_wasm_pool_size{")
                        && line.contains(&format!("module=\"{label}\""))
                        && line.ends_with(&format!(" {expected}"))),
                    "pool aggregate expected{expected}: {output}"
                );
            };
            assert_eq!(old.pool_len(), 1);
            assert_eq!(new.pool_len(), 1);
            assert_pool(2);
            let old_lease = old.acquire(100).unwrap();
            assert_pool(1);
            let new_lease = new.acquire(100).unwrap();
            assert_pool(0);
            drop(new_lease);
            assert_pool(1);
            drop(old_lease);
            assert_pool(2);
            recorder.advance(std::time::Duration::from_secs(2));
            assert!(!recorder.render().contains("edgion_wasm_pool_size"));
            drop(old_runtime);
            drop(old);
            assert!(
                !recorder.render().contains("edgion_wasm_pool_size"),
                "old generation retirement must not recreate the expired epoch"
            );
            assert_eq!(
                new.pool_len(),
                1,
                "old retirement cannot overwrite the replacement contribution"
            );
            let lease = new.acquire(100).unwrap();
            assert_pool(0);
            drop(new_runtime);
            drop(new);
            recorder.advance(std::time::Duration::from_secs(2));
            assert!(!recorder.render().contains("edgion_wasm_pool_size"));
            drop(lease);
            // Late return to a retired pool must not resurrect its gauge epoch.
            recorder.advance(std::time::Duration::from_secs(2));
            assert!(!recorder.render().contains("edgion_wasm_pool_size"));
            assert_eq!(recorder.stats().gauge_owners, 0);
            assert_eq!(recorder.stats().attached_gauge_owners, 0);
        },
    )
    .await;
}

#[tokio::test]
async fn retirement_drains_idle_guests_and_late_returns_do_not_repopulate() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "retirement_drains_idle_guests_and_late_returns_do_not_repopulate",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "retirement_drains_idle_guests_and_late_returns_do_not_repopulate",
        || async {
            let manager = WasmVmManager::new_for_test();
            let file = write_temp_wasm(MINI_GUEST);
            let runtime = actual_runtime(&manager, &file_config(file.path()), CalloutGrant::none());
            let entry = runtime.get_or_load().await.unwrap();
            let mut active = entry.acquire(100).unwrap();
            active.open_request(0, true).unwrap();
            let spare = entry.acquire(100).unwrap();
            drop(spare);
            assert_eq!(entry.pool_len(), 1);
            drop(runtime);
            assert_eq!(entry.pool_len(), 0);
            assert!(entry.retired.load(Ordering::Acquire));
            let id = active.context_id().unwrap();
            active.close_context(id).unwrap();
            drop(active);
            assert_eq!(entry.pool_len(), 0, "late healthy return cannot retain retired memory");
        },
    )
    .await;
}

#[tokio::test]
async fn engine_rejects_shared_memory_that_would_bypass_guest_admission() {
    crate::test_subprocess::assert_in_subprocess_async(
        "plugins::http::wasm::vm_manager::tests",
        "engine_rejects_shared_memory_that_would_bypass_guest_admission",
        "EDGION_R0001_ISOLATED_TEST_CHILD",
        "engine_rejects_shared_memory_that_would_bypass_guest_admission",
        || async {
            let manager = WasmVmManager::new_for_test();
            let file = write_temp_wasm("(module (memory (export \"memory\") 1 1 shared))");
            assert!(load(&manager, &file_config(file.path())).await.is_err());
        },
    )
    .await;
}

#[test]
fn process_guest_budget_survives_runtime_replacement_and_cancellation() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::wasm::vm_manager::tests",
        "process_guest_budget_survives_runtime_replacement_and_cancellation",
        "EDGION_WASM_GUEST_BUDGET_CHILD",
        "bounded",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                super::super::admission::initialize(super::super::admission::WasmLimits {
                    guest_memory_bytes: 2 * 65536,
                    max_idle_guest_memory_bytes: 2 * 65536,
                    ..Default::default()
                })
                .unwrap();
                let bytes = || {
                    serde_json::to_value(super::super::admission::stats()).unwrap()["guest_bytes"]
                        .as_u64()
                        .unwrap()
                };
                assert_eq!(bytes(), 0);
                let manager = WasmVmManager::new_for_test();
                let file = write_temp_wasm(MINI_GUEST);
                let config = file_config(file.path());
                let old_runtime = actual_runtime(&manager, &config, CalloutGrant::none());
                let old = old_runtime.get_or_load().await.unwrap();
                let mut lease = old.acquire(100).unwrap();
                lease.open_request(0, true).unwrap();
                drop(old_runtime);
                assert_eq!(bytes(), 65536, "active retired guest remains charged");
                let new_runtime = actual_runtime(&manager, &config, CalloutGrant::none());
                let new = new_runtime.get_or_load().await.unwrap();
                assert_eq!(bytes(), 2 * 65536);
                assert!(new.acquire_timer().is_err());
                assert_eq!(
                    bytes(),
                    2 * 65536,
                    "failed initialization rolls back without freeing live bytes"
                );
                drop(new_runtime);
                assert_eq!(bytes(), 65536, "retirement drains idle even when entry Arc survives");
                drop(new);
                lease.poison();
                drop(lease);
                assert_eq!(bytes(), 0);
                assert_eq!(old.pool_len(), 0);
            });
        },
    );
}

#[test]
fn all_source_preparation_rejects_before_fetch_when_full() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::wasm::vm_manager::tests",
        "all_source_preparation_rejects_before_fetch_when_full",
        "EDGION_WASM_PREPARATION_CHILD",
        "bounded",
        || {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let permits: Vec<_> = (0..super::super::admission::limits().max_preparations)
                    .map(|_| super::super::admission::try_prepare().unwrap())
                    .collect();
                assert!(super::super::admission::try_prepare().is_err());
                let manager = WasmVmManager::new_for_test();
                let file = write_temp_wasm(MINI_GUEST);
                let config = file_config(file.path());
                let error = load(&manager, &config).await.unwrap_err();
                assert!(error.contains("preparation capacity"), "{error}");
                drop(permits);
                let entry = load(&manager, &config).await.unwrap();
                assert_eq!(entry.pool_len(), 1);
                let stats = serde_json::to_value(super::super::admission::stats()).unwrap();
                assert_eq!(stats["preparations"], 0);
            });
        },
    );
}

#[test]
fn engine_freezes_supported_memory_profile_for_growth_failure_accounting() {
    let manager = WasmVmManager::new_for_test();
    let engine = manager.engine.as_ref().unwrap();
    for guest in [
        "(module (memory i64 1))",
        "(module (memory 1 (pagesize 1)))",
        "(module (memory 1 1 shared))",
    ] {
        let bytes = wat::parse_str(guest).unwrap();
        assert!(
            Module::new(engine, bytes).is_err(),
            "unsupported memory profile: {guest}"
        );
    }
}

#[test]
fn configured_idle_limits_span_entries_and_release_on_checkout_and_retirement() {
    for case in ["count", "bytes", "disabled"] {
        crate::test_subprocess::assert_in_subprocess(
            "plugins::http::wasm::vm_manager::tests",
            "configured_idle_limits_span_entries_and_release_on_checkout_and_retirement",
            "EDGION_WASM_IDLE_LIMITS_CHILD",
            case,
            || {
                tokio::runtime::Runtime::new().unwrap().block_on(async {
                    use super::super::admission::{initialize, stats, WasmLimits};
                    initialize(WasmLimits {
                        max_instances: 3,
                        max_idle_instances: if case == "count" { 1 } else { 3 },
                        max_idle_guest_memory_bytes: match case {
                            "count" => 3 * 65536,
                            "bytes" => 65536,
                            _ => 0,
                        },
                        max_instance_table_elements: 2,
                        ..Default::default()
                    })
                    .unwrap();
                    let counter = |name: &str| serde_json::to_value(stats()).unwrap()[name].as_u64().unwrap();
                    let manager = WasmVmManager::new_for_test();
                    let file = write_temp_wasm(MINI_GUEST);
                    let config = file_config(file.path());
                    let first_runtime = actual_runtime(&manager, &config, CalloutGrant::none());
                    let first = first_runtime.get_or_load().await.unwrap();
                    let second_runtime = actual_runtime(&manager, &config, CalloutGrant::none());
                    let second = second_runtime.get_or_load().await.unwrap();
                    let retained = u64::from(case != "disabled");
                    assert_eq!(first.pool_len(), retained as usize);
                    assert_eq!(second.pool_len(), 0);
                    assert_eq!(counter("idle_instances"), retained);
                    assert_eq!(counter("idle_guest_bytes"), retained * 65536);
                    let a = first.acquire(100).unwrap();
                    assert_eq!(counter("idle_instances"), 0);
                    assert_eq!(counter("idle_guest_bytes"), 0);
                    let b = second.acquire(100).unwrap();
                    let c = first.acquire(100).unwrap();
                    assert_eq!(counter("instances"), 3);
                    assert!(second.acquire(100).is_err());
                    drop(a);
                    drop(b);
                    drop(c);
                    assert_eq!(counter("instances"), retained);
                    assert_eq!(counter("idle_instances"), retained);
                    assert_eq!(counter("idle_guest_bytes"), retained * 65536);
                    drop(first_runtime);
                    drop(second_runtime);
                    assert_eq!(counter("instances"), 0);
                    assert_eq!(counter("guest_bytes"), 0);
                    assert_eq!(counter("idle_instances"), 0);
                    assert_eq!(counter("idle_guest_bytes"), 0);
                    assert_eq!(first.pool_len(), 0);
                    assert_eq!(second.pool_len(), 0);
                    let oversized_table = write_temp_wasm("(module (table 3 funcref))");
                    assert!(load(&manager, &file_config(oversized_table.path())).await.is_err());
                    assert_eq!(counter("instances"), 0);
                    assert_eq!(counter("guest_bytes"), 0);
                });
            },
        );
    }
}

#[tokio::test]
async fn url_fetch_rejects_configured_cidr_before_network_admission() {
    use crate::test_blocked_cidr::{install, BLOCKED_CIDR_TEST_IP};
    install();
    let url = format!("http://{BLOCKED_CIDR_TEST_IP}/module.wasm");
    let permits = tokio::sync::Semaphore::new(0);
    permits.close();
    let result = fetch_url_bytes_with_permits(&permits, &url, None, Some(&actual_url_client(&url, None))).await;
    assert_eq!(
        result.unwrap_err(),
        "Wasm destination blocked by configured egress CIDR"
    );
}
