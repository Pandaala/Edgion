//! One shared JIT engine and independently owned plugin runtimes.
//! Each runtime publishes its own instance pool, shared data, and tick task.
//!
//! Design:
//! - One global `Engine`, built once behind a `OnceLock`, using the pooling
//!   allocator (`InstanceAllocationStrategy::Pooling`) for fast per-request
//!   instantiation and epoch interruption (`epoch_interruption(true)`). An
//!   owned background thread increments the engine epoch every
//!   `EPOCH_TICK_MS`; each lease sets a deadline derived from the config's
//!   `timeout_ms` before running guest code, so a stuck/looping guest traps
//!   instead of hanging a worker thread. Dropping a test manager stops and
//!   joins its ticker so its Engine and pooling mappings can be released.
//! - Each plugin owns one load slot. Each slot publishes
//!   its last-known-good entry through an `ArcSwapOption`, while an async
//!   mutex serializes only the initial load / failure state. Concurrent
//!   `get_or_load` callers for a cold plugin share one in-flight compile.
//!   OCI `Always` refresh runs in the background and never makes readers
//!   wait for registry IO or compilation. A cold-load error is cached for
//!   `LOAD_RETRY_SECS` so a broken source doesn't get hammered on every
//!   request; after that window it is retried.
//! - Bytecode fetch supports `file` (read relative to the gateway work dir),
//!   `inline` (base64), `url` (plain `reqwest::get` with a fixed timeout,
//!   200-only), and `oci` (pulled via `super::oci::fetch_oci_bytes`, which is
//!   memory-cache-first; see `oci::cache` for bounded artifact retention). An `oci`
//!   source with `pullPolicy: Always` is re-checked against the registry at
//!   most every `OCI_ALWAYS_RECHECK_SECS`: request and response paths can
//!   claim one background refresh with lock-free checks. An unchanged digest
//!   keeps the cached entry; a changed digest is fully loaded before an
//!   atomic swap; and any refresh failure keeps the loaded module. When
//!   `sha256` is configured it is verified before compilation, covering
//!   OCI-fetched bytes too; a mismatch is a load error (never compiled).
//! - The optional SHA-256 check, compatibility analysis, and Wasmtime
//!   compilation run in that order inside one `spawn_blocking` task. Analysis
//!   rejects malformed/component binaries, unsupported or mixed ABI markers,
//!   and imports/hooks outside the exact host profile. All three operations
//!   may scan the full module and must not block the async executor.
//! - Per module: a `Linker` gets the proxy-wasm host functions registered
//!   via `host_abi::add_to_linker`, then
//!   `linker.define_unknown_imports_as_traps(&module)` is called *after* —
//!   compatibility analysis has already rejected unknown imports, but the trap
//!   linker remains defense in depth. It must come after `add_to_linker` so
//!   real host functions win. The resulting `InstancePre` is stored on
//!   `ModuleEntry` so new instances are cheap to spin up.
//! - Instance init sequence (`ModuleEntry::new_vm`): instantiate from
//!   `InstancePre` → optional `_initialize`/`_start` → optional
//!   `proxy_on_context_create(ROOT_CONTEXT_ID, 0)` → `proxy_on_vm_start` →
//!   `proxy_on_configure`. The last two proxy-wasm hooks return an i32 bool;
//!   `0` is treated as an init failure (not just a trap).
//! - Pool: `parking_lot::Mutex<Vec<PooledVm>>` bounded by
//!   `instance_pool_size`. `acquire` pops a warm VM or creates one fresh from
//!   `InstancePre`. `VmLease::drop` returns a *healthy* VM to the pool (up to
//!   capacity); a poisoned (trapped) or over-capacity VM is simply dropped,
//!   freeing its pooling-allocator slot.

#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use wasmtime::{
    Config, Engine, Instance, InstanceAllocationStrategy, InstancePre, Linker, Module, PoolingAllocationConfig, Store,
};

use super::callout::resolve_callouts_on;
use super::host_abi::{self, HostState, WasmPhase};
use super::shared_data::SharedData;
use crate::observe::metrics::registry::{
    record_wasm_compile_duration, record_wasm_hook_duration, record_wasm_instance_discard,
    record_wasm_instance_instantiate, record_wasm_module_load, record_wasm_trap, set_wasm_pool_size,
    wasm_module_metrics, WasmModuleMetrics,
};
use crate::plugins::runtime::task_scope::{PluginTaskHandle, PluginTaskOwner};
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::plugin_configs::{WasmConfig, WasmSourceSpec};

/// How often the background thread bumps the engine epoch. Guest deadlines
/// are expressed in multiples of this tick.
const EPOCH_TICK_MS: u64 = 10;
/// How long a load error is cached before a fresh `get_or_load` retries the
/// source instead of returning the cached failure immediately.
const LOAD_RETRY_SECS: u64 = 30;
/// Minimum interval between manifest re-resolutions for an OCI source with
/// `pullPolicy: Always`. Bounds registry traffic on hot request paths.
const OCI_ALWAYS_RECHECK_SECS: u64 = 60;
/// proxy-wasm root context id (id 1 is reserved for the VM-wide context that
/// owns `on_vm_start`/`on_configure`; per-request contexts start above it).
const ROOT_CONTEXT_ID: i32 = 1;
/// Fixed timeout for `url`-sourced bytecode fetches when the config carries
/// no `source.fetch.timeoutMs` override.
const URL_FETCH_TIMEOUT_SECS: u64 = 10;
/// Hard cap on module bytes loaded from file or URL sources. This matches the
/// OCI blob/extracted-module caps and the CLI inspector cap.
const MAX_MODULE_BYTES: usize = 64 * 1024 * 1024;
/// Floor for guest-requested tick periods (busy-loop protection).
const MIN_TICK_PERIOD_MS: u64 = 100;
/// Stable failure returned when Rust host code unwinds through Wasmtime.
/// Panic payloads may contain request or configuration data and are never
/// propagated to callers.
const WASM_HOST_PANIC_ERROR: &str = "wasm host call panicked";
const PRELOAD_TASK: usize = 0;
const REFRESH_TASK: usize = 1;

/// Run one synchronous Wasmtime guest boundary under the process-wide panic
/// redaction policy. Ordinary Wasmtime and host-function errors retain their
/// existing shape; only a Rust unwind is replaced with the fixed error above.
fn run_guest_boundary<T>(call: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    match catch_redacted_panic(call) {
        Ok(result) => result,
        Err(_) => Err(WASM_HOST_PANIC_ERROR.to_string()),
    }
}

fn add_production_host_abi(linker: &mut Linker<HostState>) -> Result<(), String> {
    host_abi::add_to_linker(linker).map_err(|e| format!("linker setup: {e}"))
}

/// Process-wide engine and epoch ticker shared by independent plugin runtimes.
pub fn global_vm_manager() -> &'static WasmVmManager {
    static GLOBAL: OnceLock<WasmVmManager> = OnceLock::new();
    GLOBAL.get_or_init(WasmVmManager::new)
}

type SharedLoadSlot = Arc<LoadSlot>;

/// One plugin-owned slot. Readers never take `state`: a fully initialized,
/// last-known-good module is published independently and remains available
/// throughout an OCI registry re-check, download, compile, or init failure.
struct LoadSlot {
    published: ArcSwapOption<ModuleEntry>,
    state: tokio::sync::Mutex<LoadState>,
    /// Monotonic process-relative millisecond deadline for an OCI `Always`
    /// re-check. `u64::MAX` disables refresh for cold/non-Always entries.
    refresh_due_ms: AtomicU64,
    /// Single-flight claim for the background refresh task. This is checked
    /// on response-body chunks, so it must stay lock-free.
    refresh_in_flight: AtomicBool,
    /// Effective OCI policy, resolved once when the plugin-owned slot is
    /// created. Hot response-body scheduling never parses the image ref.
    refresh_always: bool,
    /// Single-flight claim for best-effort preload tasks. The cold-load state
    /// mutex still serializes actual loads; this claim prevents sync response
    /// misses (especially one per body chunk) from piling up waiter tasks.
    preload_in_flight: AtomicBool,
    /// Earliest monotonic time at which a failed cold load may be preloaded
    /// again. Checked before task creation, so the negative-cache window does
    /// not create no-op tasks.
    load_retry_due_ms: AtomicU64,
}

impl LoadSlot {
    fn new(refresh_always: bool) -> Self {
        Self {
            published: ArcSwapOption::empty(),
            state: tokio::sync::Mutex::new(LoadState::NotLoaded),
            refresh_due_ms: AtomicU64::new(u64::MAX),
            refresh_in_flight: AtomicBool::new(false),
            refresh_always,
            preload_in_flight: AtomicBool::new(false),
            load_retry_due_ms: AtomicU64::new(0),
        }
    }

    fn try_claim_refresh(self: &Arc<Self>) -> Option<RefreshClaimGuard> {
        self.refresh_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| RefreshClaimGuard { slot: self.clone() })
    }

    fn try_claim_preload(self: &Arc<Self>) -> Option<PreloadClaimGuard> {
        self.preload_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| PreloadClaimGuard { slot: self.clone() })
    }
}

/// Cancellation-safe ownership of one refresh claim. Tokio task abort,
/// runtime shutdown, panic, and every early return all release the claim.
struct RefreshClaimGuard {
    slot: SharedLoadSlot,
}

impl Drop for RefreshClaimGuard {
    fn drop(&mut self) {
        self.slot.refresh_in_flight.store(false, Ordering::Release);
    }
}

/// Cancellation-safe ownership of one preload task claim.
struct PreloadClaimGuard {
    slot: SharedLoadSlot,
}

impl Drop for PreloadClaimGuard {
    fn drop(&mut self) {
        self.slot.preload_in_flight.store(false, Ordering::Release);
    }
}

/// One plugin instance's normalized callout authorization.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CalloutGrant {
    pub allowlist: Arc<[String]>,
    pub timeout_ms: u64,
}

impl CalloutGrant {
    /// Disable callouts for this plugin runtime.
    pub fn none() -> Self {
        CalloutGrant {
            allowlist: Arc::from(Vec::new()),
            timeout_ms: 0,
        }
    }
}

/// Immutable authorization owned by exactly one plugin runtime.
struct TickCalloutState {
    grant: CalloutGrant,
    #[cfg(test)]
    active_tick_tasks: AtomicUsize,
}

impl TickCalloutState {
    fn new(grant: CalloutGrant) -> Self {
        Self {
            grant,
            #[cfg(test)]
            active_tick_tasks: AtomicUsize::new(0),
        }
    }
}

#[cfg(test)]
struct TestTickTaskGuard {
    state: Arc<TickCalloutState>,
}
#[cfg(test)]
impl TestTickTaskGuard {
    fn new(state: Arc<TickCalloutState>) -> Self {
        state.active_tick_tasks.fetch_add(1, Ordering::AcqRel);
        Self { state }
    }
}
#[cfg(test)]
impl Drop for TestTickTaskGuard {
    fn drop(&mut self) {
        self.state.active_tick_tasks.fetch_sub(1, Ordering::AcqRel);
    }
}

enum LoadState {
    NotLoaded,
    Loaded {
        /// OCI digest bookkeeping (`Always` re-check); `None` for other sources.
        oci: Option<OciLoadedMeta>,
    },
    Failed {
        error: String,
    },
}

/// Digest bookkeeping for a loaded OCI-sourced module.
struct OciLoadedMeta {
    manifest_digest: String,
}

struct PreparedModule {
    entry: Arc<ModuleEntry>,
    oci: Option<OciLoadedMeta>,
    tick_period_ms: u64,
}

/// Global engine and owned epoch ticker.
///
/// `engine` is a `Result` rather than a bare `Engine`: pooling-allocator
/// reservation can fail (e.g. a restrictive `RLIMIT_AS`), and the gateway's
/// "never crash" invariant means that failure must degrade every Wasm
/// plugin to its `load`-style `fail_mode` at request time, not panic the
/// process during `OnceLock` init.
pub struct WasmVmManager {
    engine: Result<Engine, String>,
    /// Owns the epoch thread for a successfully constructed Engine. Dropped
    /// explicitly before the Engine so no thread retains an Engine
    /// clone beyond this manager's lifetime.
    epoch_ticker: Option<EpochTicker>,
}

struct EpochTicker {
    stop: std::sync::mpsc::Sender<()>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EpochTicker {
    fn spawn(engine: Engine) -> std::io::Result<Self> {
        let (stop, stopped) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("wasm-epoch-ticker".to_string())
            .spawn(move || loop {
                match stopped.recv_timeout(Duration::from_millis(EPOCH_TICK_MS)) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => engine.increment_epoch(),
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        // An unbounded channel send is non-blocking and wakes recv_timeout
        // immediately. Ignore shutdown races and thread panics during Drop.
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WasmVmManager {
    fn drop(&mut self) {
        // Stop the thread before field teardown drops the manager's Engine
        // and its Engine clones.
        drop(self.epoch_ticker.take());
    }
}

impl WasmVmManager {
    fn new() -> Self {
        Self::new_with_epoch_ticker_spawner(EpochTicker::spawn)
    }

    fn new_with_epoch_ticker_spawner(spawn_epoch_ticker: impl FnOnce(Engine) -> std::io::Result<EpochTicker>) -> Self {
        let mut cfg = Config::new();
        cfg.epoch_interruption(true);
        // Pooling allocator: fast instantiation for per-request leases (avoids
        // mmap/munmap churn on the hot path of the on-demand allocator).
        let pooling = PoolingAllocationConfig::default();
        cfg.allocation_strategy(InstanceAllocationStrategy::Pooling(pooling));
        let engine = match Engine::new(&cfg) {
            Ok(engine) => engine,
            Err(e) => {
                let msg = format!("wasm engine init failed: {e}");
                tracing::error!(error = %msg, "wasm engine unavailable; all Wasm plugins will fail-mode per configured failOpen");
                return WasmVmManager {
                    engine: Err(msg),
                    epoch_ticker: None,
                };
            }
        };
        // Owned epoch ticker: bounds guest execution time. One thread per
        // process (this constructor runs once via OnceLock in production;
        // tests use `new_for_test` and get a ticker that is stopped when that
        // manager is dropped). Only spawned once the engine itself is known
        // good; a failed engine has nothing for the ticker to advance.
        let epoch_ticker = match spawn_epoch_ticker(engine.clone()) {
            Ok(ticker) => ticker,
            Err(e) => {
                let msg = format!("wasm epoch ticker init failed: {e}");
                tracing::error!(error = %msg, "wasm engine unavailable; all Wasm plugins will fail-mode per configured failOpen");
                return WasmVmManager {
                    engine: Err(msg),
                    epoch_ticker: None,
                };
            }
        };
        WasmVmManager {
            engine: Ok(engine),
            epoch_ticker: Some(epoch_ticker),
        }
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self::new()
    }

    /// Test-only seam for Fix 1's engine-failure path: build a manager
    /// around a pre-baked engine `Result` (no real `Engine::new` call, no
    /// ticker thread) so a forced init failure can be exercised without
    /// depending on a real allocator-exhaustion scenario, which isn't
    /// portable to trigger in a test environment.
    #[cfg(test)]
    pub fn new_with_engine_result(engine: Result<Engine, String>) -> Self {
        WasmVmManager {
            engine,
            epoch_ticker: None,
        }
    }

    #[cfg(test)]
    fn engine(&self) -> Result<&Engine, String> {
        self.engine.as_ref().map_err(|e| e.clone())
    }

    pub fn new_runtime(&self, config: &WasmConfig, grant: CalloutGrant) -> WasmRuntime {
        let tasks = PluginTaskOwner::new();
        super::oci::cache::register_owner();
        WasmRuntime {
            state: Arc::new(RuntimeState {
                engine: self.engine.clone(),
                config: config.clone(),
                slot: Arc::new(LoadSlot::new(effective_oci_always(config))),
                tick_callouts: Arc::new(TickCalloutState::new(grant)),
                tasks: tasks.handle(),
            }),
            tasks,
        }
    }
}

/// One configured plugin owns its runtime. Tasks only retain `RuntimeState`.
pub struct WasmRuntime {
    state: Arc<RuntimeState>,
    tasks: PluginTaskOwner<2>,
}

struct RuntimeState {
    engine: Result<Engine, String>,
    config: WasmConfig,
    slot: SharedLoadSlot,
    tick_callouts: Arc<TickCalloutState>,
    tasks: PluginTaskHandle<2>,
}

impl Drop for WasmRuntime {
    fn drop(&mut self) {
        // Closing the publication gate first makes the slot stable even if a
        // cancelled compile completes later on a blocking worker.
        self.tasks.close();
        if let Some(entry) = self.state.slot.published.swap(None) {
            entry.stop_tick();
        }
        super::oci::cache::unregister_owner();
    }
}

impl WasmRuntime {
    pub(super) fn config(&self) -> &WasmConfig {
        &self.state.config
    }

    pub fn try_get(&self) -> Option<Arc<ModuleEntry>> {
        self.state.slot.published.load_full()
    }

    pub fn try_get_or_schedule_refresh(&self) -> Option<Arc<ModuleEntry>> {
        let entry = self.try_get()?;
        self.state.schedule_oci_refresh(entry.clone());
        Some(entry)
    }

    pub fn spawn_preload(&self) {
        self.state.spawn_preload();
    }

    pub async fn get_or_load(&self) -> Result<Arc<ModuleEntry>, String> {
        self.state.get_or_load_with_linker(add_production_host_abi).await
    }

    #[cfg(test)]
    async fn get_or_load_with_linker<F>(&self, add_host_abi: F) -> Result<Arc<ModuleEntry>, String>
    where
        F: FnOnce(&mut Linker<HostState>) -> Result<(), String> + Send,
    {
        self.state.get_or_load_with_linker(add_host_abi).await
    }
}

impl RuntimeState {
    fn prepare_preload(&self) -> Option<PreloadClaimGuard> {
        if self.engine.is_err() || tokio::runtime::Handle::try_current().is_err() {
            return None;
        }
        if self.slot.published.load_full().is_some()
            || monotonic_millis() < self.slot.load_retry_due_ms.load(Ordering::Acquire)
        {
            return None;
        }
        self.slot.try_claim_preload()
    }

    fn spawn_preload(self: &Arc<Self>) {
        let Some(claim) = self.prepare_preload() else {
            return;
        };
        // Only the winner clones task inputs; response misses stay allocation-free.
        let state = self.clone();
        self.tasks.spawn(PRELOAD_TASK, async move {
            let _claim = claim;
            if let Err(error) = state.get_or_load_with_linker(add_production_host_abi).await {
                tracing::warn!(error = %error, "wasm module preload failed");
            }
        });
    }

    async fn get_or_load_with_linker<F>(self: &Arc<Self>, add_host_abi: F) -> Result<Arc<ModuleEntry>, String>
    where
        F: FnOnce(&mut Linker<HostState>) -> Result<(), String> + Send,
    {
        let engine = self.engine.as_ref().map_err(Clone::clone)?.clone();
        if let Some(entry) = self.slot.published.load_full() {
            self.schedule_oci_refresh(entry.clone());
            return Ok(entry);
        }
        let mut guard = self.slot.state.lock().await;
        if let Some(entry) = self.slot.published.load_full() {
            return Ok(entry);
        }
        if let LoadState::Failed { error } = &*guard {
            if monotonic_millis() < self.slot.load_retry_due_ms.load(Ordering::Acquire) {
                return Err(error.clone());
            }
        }
        let label = self.config.source.metrics_label();
        match Self::load_inner_with_linker(engine, &self.config, self.tick_callouts.clone(), add_host_abi).await {
            Ok(prepared) => self
                .tasks
                .with_active(|| {
                    self.slot.load_retry_due_ms.store(u64::MAX, Ordering::Release);
                    self.slot.refresh_due_ms.store(
                        next_oci_refresh_deadline(&self.slot, prepared.oci.is_some()),
                        Ordering::Release,
                    );
                    *guard = LoadState::Loaded { oci: prepared.oci };
                    self.slot.published.store(Some(prepared.entry.clone()));
                    prepared.entry.activate_tick(prepared.tick_period_ms);
                    record_wasm_module_load(&label, "ok");
                    prepared.entry
                })
                .ok_or_else(|| "wasm plugin runtime stopped".to_string()),
            Err(error) => {
                record_wasm_module_load(&label, "error");
                self.slot.load_retry_due_ms.store(
                    monotonic_millis().saturating_add(LOAD_RETRY_SECS * 1000),
                    Ordering::Release,
                );
                *guard = LoadState::Failed { error: error.clone() };
                Err(error)
            }
        }
    }

    async fn load_inner(
        engine: Engine,
        config: &WasmConfig,
        tick_callouts: Arc<TickCalloutState>,
    ) -> Result<PreparedModule, String> {
        Self::load_inner_with_linker(engine, config, tick_callouts, add_production_host_abi).await
    }

    async fn load_inner_with_linker<F>(
        engine: Engine,
        config: &WasmConfig,
        tick_callouts: Arc<TickCalloutState>,
        add_host_abi: F,
    ) -> Result<PreparedModule, String>
    where
        F: FnOnce(&mut Linker<HostState>) -> Result<(), String> + Send,
    {
        let vm_config = decode_guest_config("vmConfig", config.vm_config.as_ref())?;
        let plugin_config = decode_guest_config("pluginConfig", config.plugin_config.as_ref())?;
        let (bytes, oci_meta, oci_pipeline) = match &config.source.oci {
            Some(spec) => {
                let fetched = super::oci::fetch_oci_bytes(spec).await?;
                (
                    fetched.bytes,
                    Some(OciLoadedMeta {
                        manifest_digest: fetched.manifest_digest,
                    }),
                    Some(fetched.pipeline),
                )
            }
            None => (
                super::oci::cache::ArtifactBytes::from(fetch_bytes(&config.source).await?),
                None,
                None,
            ),
        };
        let label = config.source.metrics_label();
        let expected_sha256 = config.sha256.clone();
        let compile_engine = engine.clone();
        // Hashing, full wasmparser validation/reporting, and Wasmtime
        // compilation are all CPU-bound and may process the full 64 MiB input.
        // Keep the entire ordered pipeline off Tokio worker threads. Measure
        // only Module::new inside the closure so the compile metric retains its
        // historical meaning.
        let compile = move || -> Result<(Module, edgion_wasm::CompatibilityReport, Duration), String> {
            if let Some(expected) = expected_sha256 {
                let actual = hex::encode(Sha256::digest(&bytes));
                if !actual.eq_ignore_ascii_case(&expected) {
                    return Err(format!("sha256 mismatch: expected {expected}, got {actual}"));
                }
            }
            let compatibility = edgion_wasm::analyze(&bytes);
            if !compatibility.compatible {
                return Err("wasm compatibility check failed".to_string());
            }
            let compile_start = Instant::now();
            let module = Module::new(&compile_engine, &bytes).map_err(|_| "wasm compile failed".to_string())?;
            Ok((
                module,
                compatibility,
                Instant::now().saturating_duration_since(compile_start),
            ))
        };
        let (module, compatibility, compile_duration) = if let Some(pipeline) = oci_pipeline {
            super::oci::blocking_stage(&pipeline, compile).await?
        } else {
            tokio::task::spawn_blocking(compile)
                .await
                .map_err(|_| "wasm validation/compile task failed".to_string())??
        };
        record_wasm_compile_duration(&label, compile_duration.as_secs_f64() * 1000.0);
        if compatibility
            .warnings
            .iter()
            .any(|warning| warning.code == edgion_wasm::CompatibilityCode::MissingAbiMarker)
        {
            tracing::warn!("wasm module exports no proxy_abi_version_* marker; loading anyway");
        }
        let has_body_hook = compatibility
            .hooks
            .iter()
            .any(|hook| hook.name == "proxy_on_response_body");
        let has_request_body_hook = compatibility
            .hooks
            .iter()
            .any(|hook| hook.name == "proxy_on_request_body");
        // Probe the body hook once at load so the per-chunk body filter can
        // skip leasing an instance for header-only guests (see
        // `has_body_hook`); export lookup on every chunk would otherwise
        // contend the pool mutex for no benefit on large streamed responses.
        let mut linker: Linker<HostState> = Linker::new(&engine);
        add_host_abi(&mut linker)?;
        // Defense in depth after the compatibility profile rejects unknown
        // imports. Must run after add_to_linker so real host functions win.
        linker
            .define_unknown_imports_as_traps(&module)
            .map_err(|e| format!("linker traps: {e}"))?;
        let instance_pre = linker
            .instantiate_pre(&module)
            .map_err(|e| format!("instantiate_pre: {e}"))?;
        let entry = Arc::new(ModuleEntry {
            _metrics: wasm_module_metrics(&label),
            engine,
            instance_pre,
            pool: Mutex::new(Vec::new()),
            pool_cap: config.instance_pool_size,
            vm_config,
            plugin_config,
            shared_data: Arc::new(SharedData::new(label.clone())),
            metrics_label: label,
            hook_timeout_ms: config.timeout_ms,
            tick_task: Mutex::new(None),
            tick_period_ms: AtomicU64::new(0),
            tick_enabled: AtomicBool::new(false),
            tick_callouts,
            has_body_hook,
            has_request_body_hook,
        });
        // Eagerly warm one instance to validate guest init at load time. Do
        // not activate tick here: refresh builds are not published yet and
        // must have no external side effects.
        let warm = entry
            .acquire_prepared(entry.hook_timeout_ms)
            .map_err(|e| format!("wasm instance init failed: {e}"))?;
        let tick_period_ms = warm.state().tick_period_ms;
        drop(warm);
        Ok(PreparedModule {
            entry,
            oci: oci_meta,
            tick_period_ms,
        })
    }

    fn schedule_oci_refresh(self: &Arc<Self>, old_entry: Arc<ModuleEntry>) {
        if monotonic_millis() < self.slot.refresh_due_ms.load(Ordering::Acquire) {
            return;
        }
        let Some(claim) = self.slot.try_claim_refresh() else {
            return;
        };
        if self.engine.is_err() {
            return;
        }
        let state = self.clone();
        self.tasks.spawn(REFRESH_TASK, async move {
            let _claim = claim;
            state.refresh_oci_entry(old_entry).await;
        });
    }

    async fn refresh_oci_entry(&self, old_entry: Arc<ModuleEntry>) {
        let previous_digest = {
            let guard = self.slot.state.lock().await;
            match &*guard {
                LoadState::Loaded { oci: Some(oci) } => oci.manifest_digest.clone(),
                _ => return,
            }
        };
        let Some(spec) = self.config.source.oci.as_ref() else {
            return;
        };
        let resolved_digest = match super::oci::resolve_manifest_digest(spec).await {
            Ok(digest) => digest,
            Err(error) => {
                tracing::warn!(error = %error, "wasm oci Always re-check failed; keeping the loaded module");
                finish_oci_refresh(&self.slot);
                return;
            }
        };
        if resolved_digest == previous_digest {
            finish_oci_refresh(&self.slot);
            return;
        }
        let Ok(engine) = &self.engine else {
            return;
        };
        let label = self.config.source.metrics_label();
        let prepared = match Self::load_inner(engine.clone(), &self.config, self.tick_callouts.clone()).await {
            Ok(prepared) => prepared,
            Err(error) => {
                record_wasm_module_load(&label, "error");
                tracing::warn!(error = %error, "wasm oci Always refresh failed; keeping the loaded module");
                finish_oci_refresh(&self.slot);
                return;
            }
        };
        let mut state = self.slot.state.lock().await;
        self.tasks.with_active(|| {
            // No await between stopping the old task and publishing its successor.
            // A running old guest may finish its bounded call during the swap.
            old_entry.stop_tick();
            self.slot.refresh_due_ms.store(
                next_oci_refresh_deadline(&self.slot, prepared.oci.is_some()),
                Ordering::Release,
            );
            *state = LoadState::Loaded { oci: prepared.oci };
            self.slot.published.store(Some(prepared.entry.clone()));
            prepared.entry.activate_tick(prepared.tick_period_ms);
            record_wasm_module_load(&label, "ok");
        });
    }
}

#[cfg(test)]
impl WasmRuntime {
    async fn backdate_oci_check_for_test(&self) {
        self.state.slot.refresh_due_ms.store(0, Ordering::Release);
    }
    async fn wait_for_oci_refresh_for_test(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.slot.refresh_in_flight.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("OCI refresh finishes within deadline");
    }
    fn backdate_load_retry_for_test(&self) {
        self.state.slot.load_retry_due_ms.store(0, Ordering::Release);
    }
    async fn wait_for_preload_for_test(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.state.slot.preload_in_flight.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("preload finishes within deadline");
    }
}

/// Whether this source's effective OCI pull policy requires periodic
/// re-resolution. Admission validates the image reference; a residual parse
/// failure safely disables refresh rather than affecting an already-loaded
/// module.
fn effective_oci_pull_policy_key(config: &WasmConfig) -> &'static str {
    use edgion_resources::plugin_configs::{OciImageRef, OciPullPolicy};
    let Some(spec) = &config.source.oci else {
        return "none";
    };
    let Ok(image) = OciImageRef::parse(&spec.image) else {
        return "invalid";
    };
    match spec.effective_pull_policy(&image) {
        OciPullPolicy::Always => "always",
        OciPullPolicy::IfNotPresent => "if-not-present",
    }
}

fn effective_oci_always(config: &WasmConfig) -> bool {
    effective_oci_pull_policy_key(config) == "always"
}

/// Process-monotonic clock encoded as milliseconds for atomic deadlines.
fn monotonic_millis() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let elapsed = Instant::now()
        .saturating_duration_since(*ORIGIN.get_or_init(Instant::now))
        .as_millis();
    elapsed.min(u64::MAX as u128) as u64
}

fn next_oci_refresh_deadline(slot: &LoadSlot, has_oci_meta: bool) -> u64 {
    if has_oci_meta && slot.refresh_always {
        monotonic_millis().saturating_add(OCI_ALWAYS_RECHECK_SECS.saturating_mul(1000))
    } else {
        u64::MAX
    }
}

fn finish_oci_refresh(slot: &LoadSlot) {
    slot.refresh_due_ms
        .store(next_oci_refresh_deadline(slot, true), Ordering::Release);
}

async fn fetch_bytes(source: &WasmSourceSpec) -> Result<Vec<u8>, String> {
    if let Some(path) = &source.file {
        // Relative paths resolve against the gateway work dir, same as other
        // file-backed configs (access log LocalFile fallback, etc.).
        let resolved = edgion_resources::work_dir().resolve(path);
        return read_file_bounded(&resolved).await;
    }
    if let Some(b64) = &source.inline {
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("decode inline wasm: {e}"))?;
        if decoded.len() > edgion_resources::plugin_configs::INLINE_WARN_DECODED_BYTES {
            tracing::warn!(
                bytes = decoded.len(),
                "large inline wasm module: every conf_sync carries it in full; prefer a file or url source"
            );
        }
        return Ok(decoded);
    }
    if let Some(url) = &source.url {
        return fetch_url_bytes(url, source.fetch.as_ref()).await;
    }
    Err("wasm source has no file/inline/url".to_string())
}

#[derive(Debug)]
enum BoundedFileReadError {
    Io(std::io::Error),
    TooLarge,
}

async fn read_file_bounded(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("read wasm file '{}': {e}", path.display()))?;
    // This metadata check cheaply rejects large and sparse files. It is only
    // an optimization: the bounded read below uses the already-open file and
    // remains authoritative if the file grows after this check.
    let metadata = file
        .metadata()
        .await
        .map_err(|e| format!("read wasm file '{}': {e}", path.display()))?;
    if metadata.len() > MAX_MODULE_BYTES as u64 {
        return Err(module_too_large_error());
    }
    let initial_capacity = usize::try_from(metadata.len())
        .unwrap_or(MAX_MODULE_BYTES)
        .min(MAX_MODULE_BYTES);
    match read_open_file_bounded(file, initial_capacity, MAX_MODULE_BYTES).await {
        Ok(bytes) => Ok(bytes),
        Err(BoundedFileReadError::TooLarge) => Err(module_too_large_error()),
        Err(BoundedFileReadError::Io(error)) => Err(format!("read wasm file '{}': {error}", path.display())),
    }
}

async fn read_open_file_bounded<R>(
    reader: R,
    initial_capacity: usize,
    max_bytes: usize,
) -> Result<Vec<u8>, BoundedFileReadError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt as _;

    let mut bytes = Vec::with_capacity(initial_capacity.min(max_bytes));
    let limit = u64::try_from(max_bytes).unwrap_or(u64::MAX).saturating_add(1);
    reader
        .take(limit)
        .read_to_end(&mut bytes)
        .await
        .map_err(BoundedFileReadError::Io)?;
    if bytes.len() > max_bytes {
        return Err(BoundedFileReadError::TooLarge);
    }
    Ok(bytes)
}

fn module_too_large_error() -> String {
    format!("wasm module exceeds {MAX_MODULE_BYTES} bytes")
}

/// Bound on concurrently in-flight URL/OCI fetches across all plugins.
/// Each plugin serializes its own cold loads; this also caps fetches from
/// independent plugins, including plugins configured with the same URL.
pub(super) fn url_fetch_permits() -> &'static tokio::sync::Semaphore {
    static PERMITS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    PERMITS.get_or_init(|| tokio::sync::Semaphore::new(4))
}

/// Fetch `url` with the optional `source.fetch` hardening: custom TLS
/// (CA/mTLS via controller-resolved Secrets), an `Authorization` header from
/// a resolved Secret, per-attempt timeout, and retry with exponential
/// backoff. Retry policy is fixed: transport errors and 5xx responses retry;
/// 4xx never does (an auth failure will not heal, and retrying it only
/// floods the artifact repository's logs). Total failure surfaces one error
/// to `get_or_load`, which negative-caches it for `LOAD_RETRY_SECS`.
async fn fetch_url_bytes(
    url: &str,
    fetch: Option<&edgion_resources::plugin_configs::WasmFetchSpec>,
) -> Result<Vec<u8>, String> {
    fetch_url_bytes_with_permits(url_fetch_permits(), url, fetch).await
}

async fn fetch_url_bytes_with_permits(
    permits: &tokio::sync::Semaphore,
    url: &str,
    fetch: Option<&edgion_resources::plugin_configs::WasmFetchSpec>,
) -> Result<Vec<u8>, String> {
    use crate::link_sys::http_client::{build_outbound_client, OutboundClientConfig, OutboundRedirect};
    use crate::plugins::http::common::outbound::egress::{
        default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
    };
    use edgion_common::dns::ResolverSpec;

    let timeout = Duration::from_millis(fetch.map(|f| f.timeout_ms).unwrap_or(URL_FETCH_TIMEOUT_SECS * 1000));
    let tls = fetch.and_then(|f| f.tls.clone());
    // Build the fetch client through the shared outbound policy: redirect-Deny
    // (a followed redirect is an SSRF surface — and would forward the resolved
    // Authorization header on a same-host https->http downgrade) plus an
    // `EgressResolver` that classifies the exact IPs reqwest connects to,
    // closing the DNS-rebinding window on the artifact-repo hostname. The
    // per-attempt `.timeout(timeout)` below still governs the configured budget.
    let egress: Arc<dyn reqwest::dns::Resolve> = Arc::new(EgressResolver::new(
        ResolverSpec::System,
        EgressPolicy {
            block_private: default_block_private(),
        },
        egress_dns_cache_ttl(),
    ));
    let client = build_outbound_client(OutboundClientConfig {
        pool_max_idle_per_host: Some(4),
        connect_timeout: Some(Duration::from_secs(5)),
        total_timeout: Some(timeout),
        redirect: OutboundRedirect::Deny,
        disable_proxy: true,
        tls,
        tls_context: Some("WasmFetch"),
        dns_resolver: Some(egress),
        ..Default::default()
    })
    .map_err(|e| format!("wasm fetch client: {e}"))?;

    let auth_header = match fetch {
        Some(f) if f.auth_header_secret_ref.is_some() => match &f.resolved_auth_header {
            Some(v) => Some(v.clone()),
            // Fail the load rather than silently fetching unauthenticated:
            // the controller has not (yet) resolved the referenced Secret.
            None => return Err("wasm fetch auth header secret not resolved".to_string()),
        },
        _ => None,
    };

    let attempts = fetch.and_then(|f| f.retry.as_ref()).map(|r| r.attempts).unwrap_or(1);
    let backoff_ms = fetch.and_then(|f| f.retry.as_ref()).map(|r| r.backoff_ms).unwrap_or(0);

    let mut last_error = String::new();
    for attempt in 0..attempts {
        if attempt > 0 {
            let delay = backoff_ms.saturating_mul(1u64 << (attempt - 1).min(10));
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let permit = permits
            .acquire()
            .await
            .map_err(|_| "wasm fetch semaphore closed".to_string())?;
        let mut req = client.get(url).timeout(timeout);
        if let Some(auth) = &auth_header {
            req = req.header(reqwest::header::AUTHORIZATION, auth.clone());
        }
        let response = req.send().await;
        match response {
            Err(e) => {
                let kind = if e.is_timeout() {
                    "request timed out"
                } else if e.is_connect() {
                    "connection failed"
                } else {
                    "request failed"
                };
                last_error = format!("fetch wasm url: {kind}");
            }
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    // Fast-reject an honest oversized Content-Length before
                    // opening the body stream; the streaming counter below still
                    // guards absent/false/chunked lengths.
                    if resp.content_length().is_some_and(|cl| cl > MAX_MODULE_BYTES as u64) {
                        return Err(format!("fetch wasm url: body exceeds {MAX_MODULE_BYTES} bytes"));
                    }
                    let (body, truncated) =
                        crate::plugins::http::common::outbound::call::stream_body_capped(resp, MAX_MODULE_BYTES)
                            .await
                            .map_err(|_| "read wasm body: response error".to_string())?;
                    if truncated {
                        return Err(format!("fetch wasm url: body exceeds {MAX_MODULE_BYTES} bytes"));
                    }
                    return Ok(body.to_vec());
                }
                last_error = format!("fetch wasm url: status {status}");
                if status.is_client_error() {
                    // 4xx is definitive — do not retry.
                    return Err(last_error);
                }
            }
        }
        // Response streaming/error classification for this attempt is done;
        // release before any retry backoff or next permit competition.
        drop(permit);
    }
    Err(last_error)
}

/// A compiled module plus its instance pool. Cheaply shared (`Arc`) across
/// concurrent requests once loaded.
pub struct ModuleEntry {
    _metrics: WasmModuleMetrics,
    engine: Engine,
    instance_pre: InstancePre<HostState>,
    pool: Mutex<Vec<PooledVm>>,
    pool_cap: usize,
    vm_config: Vec<u8>,
    plugin_config: Vec<u8>,
    /// Human-readable source descriptor for metrics labels.
    metrics_label: String,
    /// Module-wide shared KV, cloned into every pooled VM's HostState.
    /// Freed after the plugin and its last in-flight lease release this entry.
    shared_data: Arc<SharedData>,
    /// Per-hook budget for background (tick) leases; mirrors the config's
    /// `timeoutMs`.
    hook_timeout_ms: u64,
    /// Background `proxy_on_tick` driver, started once when a guest sets a
    /// nonzero tick period. Holds a `Weak` back-reference; aborted on Drop.
    tick_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Latest trusted guest-selected tick period.
    tick_period_ms: AtomicU64,
    /// Publication/lifecycle gate. Prepared entries and entries being stopped
    /// for refresh cannot spawn a replacement task from a concurrent acquire.
    tick_enabled: AtomicBool,
    /// Immutable authorization from the owning plugin configuration.
    tick_callouts: Arc<TickCalloutState>,
    /// Whether the guest exports `proxy_on_response_body`. Computed once
    /// at load; lets the body-filter stage skip the per-chunk lease entirely
    /// for header-only guests.
    has_body_hook: bool,
    /// Whether the guest exports `proxy_on_request_body`. Computed once at
    /// load; lets the request-body-filter stage skip the per-chunk lease
    /// entirely for header-only guests.
    has_request_body_hook: bool,
}

impl Drop for ModuleEntry {
    fn drop(&mut self) {
        if let Some(handle) = self.tick_task.lock().take() {
            handle.abort();
        }
    }
}

// Manual impl: `Store<HostState>` inside `PooledVm` isn't `Debug`, but
// callers (notably `Result::unwrap_err` in tests) need `Arc<ModuleEntry>` to
// satisfy `Debug` when it appears in the `Ok` side of a `Result`.
impl std::fmt::Debug for ModuleEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModuleEntry")
            .field("pool_cap", &self.pool_cap)
            .field("pool_len", &self.pool.lock().len())
            .finish_non_exhaustive()
    }
}

struct PooledVm {
    store: Store<HostState>,
    instance: Instance,
    next_ctx_id: i32,
}

impl ModuleEntry {
    #[cfg(test)]
    pub fn pool_len(&self) -> usize {
        self.pool.lock().len()
    }

    /// Lease an initialized VM: pop a warm one from the pool or instantiate
    /// fresh from `InstancePre`. `timeout_ms` bounds the guest calls made
    /// through the returned lease (via epoch interruption).
    pub fn acquire(self: &Arc<Self>, timeout_ms: u64) -> Result<VmLease, String> {
        self.acquire_inner(timeout_ms, true)
    }

    /// Eager initialization for an entry that is not published yet. Guest
    /// initialization runs normally, but a configured tick period is merely
    /// observed by the caller and cannot start background work.
    fn acquire_prepared(self: &Arc<Self>, timeout_ms: u64) -> Result<VmLease, String> {
        self.acquire_inner(timeout_ms, false)
    }

    fn acquire_inner(self: &Arc<Self>, timeout_ms: u64, activate_tick: bool) -> Result<VmLease, String> {
        let (vm, pool_len) = {
            let mut pool = self.pool.lock();
            let vm = pool.pop();
            (vm, pool.len())
        };
        set_wasm_pool_size(&self.metrics_label, pool_len);
        let vm = match vm {
            Some(vm) => vm,
            None => {
                record_wasm_instance_instantiate(&self.metrics_label);
                let vm = self.new_vm(timeout_ms)?;
                // A guest typically requests its tick period during
                // on_vm_start/on_configure, which just ran inside new_vm.
                let period = vm.store.data().tick_period_ms;
                if activate_tick {
                    self.maybe_start_tick(period);
                }
                vm
            }
        };
        // The epoch deadline is armed per hook call (see `call_headers_hook`),
        // not here: `set_epoch_deadline` is relative to the current epoch,
        // and a POOLED instance can sit idle in `ModuleEntry::pool` for an
        // arbitrary amount of time between requests while the background
        // ticker keeps advancing the engine epoch, so a deadline armed once
        // here (at `acquire` time) would often already be stale by the time
        // this lease's hook call actually runs.
        Ok(VmLease {
            entry: self.clone(),
            vm: Some(vm),
            timeout_ms,
            healthy: true,
            open_context: None,
        })
    }

    /// Start the module tick task once a guest requested a nonzero period.
    /// Idempotent; requires a tokio runtime (skipped otherwise, e.g. in
    /// sync-only unit tests). The task holds only a `Weak` back-reference,
    /// while the plugin owner explicitly aborts it when shutting down.
    fn activate_tick(self: &Arc<Self>, period_ms: u64) {
        self.tick_enabled.store(true, Ordering::Release);
        self.maybe_start_tick(period_ms);
    }

    fn maybe_start_tick(self: &Arc<Self>, period_ms: u64) {
        if !self.tick_enabled.load(Ordering::Acquire)
            || period_ms == 0
            || tokio::runtime::Handle::try_current().is_err()
        {
            return;
        }
        let mut guard = self.tick_task.lock();
        if !self.tick_enabled.load(Ordering::Acquire) || guard.is_some() {
            return;
        }
        self.tick_period_ms.store(period_ms, Ordering::Release);
        let weak = Arc::downgrade(self);
        let label = self.metrics_label.clone();
        #[cfg(test)]
        let test_tick_state = self.tick_callouts.clone();
        let handle = tokio::spawn(async move {
            #[cfg(test)]
            let _test_task = TestTickTaskGuard::new(test_tick_state);
            let mut period = period_ms.max(MIN_TICK_PERIOD_MS);
            loop {
                tokio::time::sleep(Duration::from_millis(period)).await;
                let Some(entry) = weak.upgrade() else { break };
                let mut lease = match entry.acquire(entry.hook_timeout_ms) {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!(error = %e, module = %label, "wasm tick: instance acquire failed");
                        continue;
                    }
                };
                lease.state_mut().begin_phase(
                    WasmPhase::Tick,
                    Vec::new(),
                    Vec::new(),
                    std::collections::HashMap::new(),
                );
                let grant = &entry.tick_callouts.grant;
                let armed = if grant.allowlist.is_empty() {
                    None
                } else {
                    lease
                        .state_mut()
                        .arm_dispatch(grant.allowlist.clone(), grant.timeout_ms);
                    Some(grant.timeout_ms)
                };
                match lease.call_on_tick() {
                    Err(e) => {
                        tracing::warn!(error = %e, module = %label, "wasm on_tick trapped");
                        // The failed guest may have partially mutated its
                        // HostState. Discard it without reading that state and
                        // retry at the last trusted period with a fresh VM.
                        drop(lease);
                        continue;
                    }
                    Ok(()) => {
                        // Deliver callouts the guest dispatched from on_tick.
                        // The root context receives the callbacks; it exists
                        // for the whole VM lifetime, so unlike the request
                        // path there is no context to open/close here.
                        if let Some(timeout_ms) = armed {
                            if !lease.state().pending_callouts.is_empty() {
                                // Cancellation safety: eviction aborts this task
                                // at an await point inside callout resolution,
                                // which drops the lease with guest state mid-
                                // round (a taken pending queue, a callout
                                // response staged for delivery). Poison first so
                                // that drop discards the VM instead of pooling
                                // it; healthy state is restored only on a clean
                                // round. The root context needs no close hook,
                                // so `open_context` cannot cover this like it
                                // covers the request path.
                                lease.poison();
                                match resolve_callouts_on(&mut lease, ROOT_CONTEXT_ID, timeout_ms, &label).await {
                                    Ok(outcome) => {
                                        // The round completed without a trap or
                                        // cancellation: the VM is reusable again.
                                        lease.healthy = true;
                                        tracing::debug!(
                                            delivered = outcome.delivered,
                                            leftover = outcome.leftover,
                                            module = %label,
                                            "wasm tick callouts resolved"
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, module = %label, "wasm tick callout delivery failed");
                                        // Callback failure poisons the lease;
                                        // no guest-controlled state is trusted
                                        // after this point.
                                        drop(lease);
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                // The guest may adjust (or stop) its own tick period.
                let next = lease.state().tick_period_ms;
                drop(lease);
                entry.tick_period_ms.store(next, Ordering::Release);
                if next == 0 {
                    entry.tick_enabled.store(false, Ordering::Release);
                    tracing::info!(module = %label, "wasm tick stopped by guest (period 0)");
                    break;
                }
                period = next.max(MIN_TICK_PERIOD_MS);
            }
        });
        *guard = Some(handle);
    }

    /// Stop future tick work without invalidating in-flight request leases.
    fn stop_tick(&self) {
        let handle = {
            let mut task = self.tick_task.lock();
            self.tick_enabled.store(false, Ordering::Release);
            task.take()
        };
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    /// Whether the guest exports `proxy_on_response_body`. The body
    /// filter stage checks this after the cheap `try_get` lookup and returns
    /// early for header-only guests, avoiding a per-chunk pool lease.
    pub fn has_body_hook(&self) -> bool {
        self.has_body_hook
    }

    /// Whether the guest exports `proxy_on_request_body`. The request-body
    /// phase checks this after the cheap `try_get` lookup and returns early
    /// for header-only guests, skipping the pool-lease acquisition and body
    /// buffering entirely — unlike the response-body filter, this phase
    /// drives the hook exactly once with the whole buffered body (buffer
    /// mode, no per-chunk streaming), so there is no per-chunk lease to
    /// avoid here.
    pub fn has_request_body_hook(&self) -> bool {
        self.has_request_body_hook
    }

    /// Human-readable module descriptor (metrics label).
    pub fn metrics_label(&self) -> &str {
        &self.metrics_label
    }

    #[cfg(test)]
    pub fn shared_data_for_test(&self) -> &SharedData {
        &self.shared_data
    }

    fn new_vm(&self, timeout_ms: u64) -> Result<PooledVm, String> {
        let mut store = Store::new(
            &self.engine,
            HostState::new(
                self.vm_config.clone(),
                self.plugin_config.clone(),
                self.metrics_label.clone(),
                self.shared_data.clone(),
            ),
        );
        store.set_epoch_deadline(timeout_ms / EPOCH_TICK_MS + 2);
        // Cap guest linear-memory/table growth (defense in depth). The pooling
        // allocator's default per-instance memory cap is effectively the full
        // wasm32 4 GiB; `HostState`'s `ResourceLimiter` impl keeps a guest — and
        // therefore each `read_guest` transient copy sized by its memory —
        // within `MAX_GUEST_MEMORY_BYTES`. Must be set before instantiation so
        // the initial memory allocation is also limited.
        store.limiter(|state| state as &mut dyn wasmtime::ResourceLimiter);
        let instance = run_guest_boundary(|| {
            self.instance_pre
                .instantiate(&mut store)
                .map_err(|e| format!("instantiate: {e}"))
        })?;
        // Optional module initializers (WASI-style reactors/commands). Not
        // every guest exports one; absence is not an error.
        for init in ["_initialize", "_start"] {
            if let Ok(f) = instance.get_typed_func::<(), ()>(&mut store, init) {
                run_guest_boundary(|| f.call(&mut store, ()).map_err(|e| format!("{init}: {e}")))?;
                break;
            }
        }
        // proxy-wasm lifecycle: root context -> vm_start -> configure.
        if let Ok(f) = instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create") {
            run_guest_boundary(|| {
                f.call(&mut store, (ROOT_CONTEXT_ID, 0))
                    .map_err(|e| format!("proxy_on_context_create(root): {e}"))
            })?;
        }
        if let Ok(f) = instance.get_typed_func::<(i32, i32), i32>(&mut store, "proxy_on_vm_start") {
            let ok = run_guest_boundary(|| {
                f.call(&mut store, (ROOT_CONTEXT_ID, self.vm_config.len() as i32))
                    .map_err(|e| format!("proxy_on_vm_start: {e}"))
            })?;
            if ok == 0 {
                return Err("proxy_on_vm_start returned false".to_string());
            }
        }
        if let Ok(f) = instance.get_typed_func::<(i32, i32), i32>(&mut store, "proxy_on_configure") {
            let ok = run_guest_boundary(|| {
                f.call(&mut store, (ROOT_CONTEXT_ID, self.plugin_config.len() as i32))
                    .map_err(|e| format!("proxy_on_configure: {e}"))
            })?;
            if ok == 0 {
                return Err("proxy_on_configure returned false".to_string());
            }
        }
        Ok(PooledVm {
            store,
            instance,
            next_ctx_id: ROOT_CONTEXT_ID + 1,
        })
    }
}

fn decode_guest_config(
    field_name: &str,
    raw: Option<&edgion_resources::plugin_configs::WasmRawConfig>,
) -> Result<Vec<u8>, String> {
    raw.map_or_else(
        || Ok(Vec::new()),
        |value| value.guest_bytes().map_err(|error| format!("{field_name} {error}")),
    )
}

/// A leased VM, borrowed from a `ModuleEntry` pool. Returns to the pool on
/// drop unless poisoned (a trap or protocol violation during the lease).
pub struct VmLease {
    entry: Arc<ModuleEntry>,
    vm: Option<PooledVm>,
    /// Per-hook wall-clock budget, re-armed at the start of every hook call
    /// (see `call_headers_hook`) since the pooled instance backing this
    /// lease may have sat idle in `ModuleEntry::pool` for an arbitrary
    /// amount of time before being acquired, which would leave a deadline
    /// armed once at `acquire` time already stale relative to the
    /// continuously-advancing engine epoch.
    timeout_ms: u64,
    healthy: bool,
    /// Request context intentionally kept alive across async callout
    /// resolution. If the owning future is cancelled before `close_context`,
    /// the VM must be discarded rather than pooled with abandoned guest
    /// state. Drop never calls guest lifecycle hooks.
    open_context: Option<i32>,
}

impl VmLease {
    /// Mark this lease unhealthy so `Drop` discards the VM instead of
    /// returning it to the pool. Public so callers (the Task-5 plugin) can
    /// poison on their own detected failure modes too.
    pub fn poison(&mut self) {
        self.healthy = false;
    }

    fn vm_mut(&mut self) -> &mut PooledVm {
        self.vm.as_mut().expect("lease taken")
    }

    /// Allocate a fresh per-request context id (never reuses `ROOT_CONTEXT_ID`).
    pub fn next_context_id(&mut self) -> i32 {
        let vm = self.vm_mut();
        vm.next_ctx_id = vm.next_ctx_id.wrapping_add(1).max(2);
        vm.next_ctx_id
    }

    /// Run the full request-headers hook sequence for a fresh context and
    /// close it immediately. Any trap poisons the lease. Returns the guest
    /// Action (0=Continue). Used when the caller does not need the context
    /// to outlive the hook (no callout support on this path).
    pub fn call_on_request_headers(&mut self, num_headers: i32) -> Result<i32, String> {
        let (ctx_id, action) = self.open_and_call("proxy_on_request_headers", "request_headers", num_headers)?;
        self.close_context(ctx_id)?;
        Ok(action)
    }

    /// Same for response headers (always sync, context never outlives the hook).
    pub fn call_on_response_headers(&mut self, num_headers: i32) -> Result<i32, String> {
        let (ctx_id, action) = self.open_and_call("proxy_on_response_headers", "response_headers", num_headers)?;
        self.close_context(ctx_id)?;
        Ok(action)
    }

    /// Read-only response-body observation hook. The current chunk must be
    /// staged in `HostState::body_chunk` (exposed to the guest via
    /// `get_buffer_bytes(HttpResponseBody)`). A guest without the hook is a
    /// no-op (`Ok(0)`), checked before any context is opened. The returned
    /// Action is informational only — Pause cannot buffer in the read-only
    /// form and is ignored by the caller.
    pub fn call_on_response_body(&mut self, body_size: i32, end_of_stream: bool) -> Result<i32, String> {
        {
            let vm = self.vm.as_mut().expect("lease taken");
            if vm
                .instance
                .get_typed_func::<(i32, i32, i32), i32>(&mut vm.store, "proxy_on_response_body")
                .is_err()
            {
                return Ok(0);
            }
        }
        let (ctx_id, action) = self.open_and_call_with(
            "proxy_on_response_body",
            "response_body",
            body_size,
            end_of_stream as i32,
        )?;
        self.close_context(ctx_id)?;
        Ok(action)
    }

    /// Drive `proxy_on_request_body` once in buffer mode. The full request
    /// body must already be staged in `HostState::request_body` (exposed to
    /// the guest via `get_buffer_bytes(HttpRequestBody)`). A guest without
    /// the hook is a no-op (`Ok(0)`), checked before any context is opened.
    pub fn call_on_request_body(&mut self, body_size: i32, end_of_stream: bool) -> Result<i32, String> {
        {
            let vm = self.vm.as_mut().expect("lease taken");
            if vm
                .instance
                .get_typed_func::<(i32, i32, i32), i32>(&mut vm.store, "proxy_on_request_body")
                .is_err()
            {
                return Ok(0);
            }
        }
        let (ctx_id, action) =
            self.open_and_call_with("proxy_on_request_body", "request_body", body_size, end_of_stream as i32)?;
        self.close_context(ctx_id)?;
        Ok(action)
    }

    /// Open a fresh context and run the request-headers hook WITHOUT closing
    /// the context, so a paused guest can later receive
    /// `proxy_on_http_call_response` on it. The caller must eventually call
    /// `close_context` (any outcome, including local response).
    pub fn call_on_request_headers_open(&mut self, num_headers: i32) -> Result<(i32, i32), String> {
        if self.open_context.is_some() {
            self.poison();
            return Err("request context already open on this lease".to_string());
        }
        let result = self.open_and_call("proxy_on_request_headers", "request_headers", num_headers);
        if let Ok((ctx_id, _)) = &result {
            self.open_context = Some(*ctx_id);
        }
        result
    }

    /// Deliver one callout result to the paused guest. The response payload
    /// must already be staged in `HostState::callout_response`; proxy-wasm
    /// signals failure as `num_headers == 0`.
    pub fn call_on_http_call_response(
        &mut self,
        ctx_id: i32,
        token: u32,
        num_headers: i32,
        body_size: i32,
    ) -> Result<(), String> {
        let hook_start = Instant::now();
        let timeout_ms = self.timeout_ms;
        let result = (|| -> Result<(), String> {
            let vm = self.vm.as_mut().expect("lease taken");
            vm.store.set_epoch_deadline(timeout_ms / EPOCH_TICK_MS + 2);
            // SDKs route by token internally; the context id parameter names
            // the dispatching context.
            let f = vm
                .instance
                .get_typed_func::<(i32, i32, i32, i32, i32), ()>(&mut vm.store, "proxy_on_http_call_response")
                .map_err(|e| format!("guest dispatched a callout but exports no proxy_on_http_call_response: {e}"))?;
            run_guest_boundary(|| {
                f.call(&mut vm.store, (ctx_id, token as i32, num_headers, body_size, 0))
                    .map_err(|e| format!("proxy_on_http_call_response trapped: {e}"))
            })?;
            Ok(())
        })();
        record_wasm_hook_duration(
            &self.entry.metrics_label,
            "http_call_response",
            Instant::now().saturating_duration_since(hook_start).as_secs_f64() * 1000.0,
        );
        if result.is_err() {
            record_wasm_trap(&self.entry.metrics_label, "http_call_response");
            self.poison();
        }
        result
    }

    /// Invoke `proxy_on_tick(ROOT_CONTEXT_ID)`. A guest without the export
    /// is a no-op (it may have set a tick period through a shared SDK path
    /// without implementing the callback).
    pub fn call_on_tick(&mut self) -> Result<(), String> {
        let hook_start = Instant::now();
        let timeout_ms = self.timeout_ms;
        let result = (|| -> Result<(), String> {
            let vm = self.vm.as_mut().expect("lease taken");
            vm.store.set_epoch_deadline(timeout_ms / EPOCH_TICK_MS + 2);
            let Ok(f) = vm.instance.get_typed_func::<i32, ()>(&mut vm.store, "proxy_on_tick") else {
                return Ok(());
            };
            run_guest_boundary(|| {
                f.call(&mut vm.store, ROOT_CONTEXT_ID)
                    .map_err(|e| format!("proxy_on_tick trapped: {e}"))
            })
        })();
        record_wasm_hook_duration(
            &self.entry.metrics_label,
            "tick",
            Instant::now().saturating_duration_since(hook_start).as_secs_f64() * 1000.0,
        );
        if result.is_err() {
            record_wasm_trap(&self.entry.metrics_label, "tick");
            self.poison();
        }
        result
    }

    /// Close a context opened by `call_on_request_headers_open`
    /// (`proxy_on_done` + `proxy_on_delete`). Failures poison the lease.
    pub fn close_context(&mut self, ctx_id: i32) -> Result<(), String> {
        let timeout_ms = self.timeout_ms;
        let result = (|| -> Result<(), String> {
            let vm = self.vm.as_mut().expect("lease taken");
            vm.store.set_epoch_deadline(timeout_ms / EPOCH_TICK_MS + 2);
            if let Ok(f) = vm.instance.get_typed_func::<i32, i32>(&mut vm.store, "proxy_on_done") {
                run_guest_boundary(|| f.call(&mut vm.store, ctx_id).map_err(|e| format!("proxy_on_done: {e}")))?;
            }
            if let Ok(f) = vm.instance.get_typed_func::<i32, ()>(&mut vm.store, "proxy_on_delete") {
                run_guest_boundary(|| {
                    f.call(&mut vm.store, ctx_id)
                        .map_err(|e| format!("proxy_on_delete: {e}"))
                })?;
            }
            Ok(())
        })();
        if result.is_err() {
            record_wasm_trap(&self.entry.metrics_label, "context_close");
            self.poison();
        } else if self.open_context == Some(ctx_id) {
            self.open_context = None;
        }
        result
    }

    fn open_and_call(&mut self, hook: &str, phase_label: &'static str, num_headers: i32) -> Result<(i32, i32), String> {
        self.open_and_call_with(hook, phase_label, num_headers, 1)
    }

    /// Open a fresh context and invoke `hook(ctx_id, arg2, arg3)`. Header
    /// hooks pass (num_headers, end_of_stream=1); the body hook passes
    /// (body_size, end_of_stream).
    fn open_and_call_with(
        &mut self,
        hook: &str,
        phase_label: &'static str,
        arg2: i32,
        arg3: i32,
    ) -> Result<(i32, i32), String> {
        let hook_start = Instant::now();
        let ctx_id = self.next_context_id();
        let timeout_ms = self.timeout_ms;
        let result = (|| -> Result<i32, String> {
            let vm = self.vm.as_mut().expect("lease taken");
            // Re-arm relative to the CURRENT epoch: the engine epoch advances
            // continuously via the background ticker even while this
            // instance sits idle in the pool, so a deadline armed once at
            // `acquire` time would often already be stale by the time this
            // hook call actually runs. `+2` ticks of slack so a call right
            // before a tick boundary still gets ~timeout_ms.
            vm.store.set_epoch_deadline(timeout_ms / EPOCH_TICK_MS + 2);
            if let Ok(f) = vm
                .instance
                .get_typed_func::<(i32, i32), ()>(&mut vm.store, "proxy_on_context_create")
            {
                run_guest_boundary(|| {
                    f.call(&mut vm.store, (ctx_id, ROOT_CONTEXT_ID))
                        .map_err(|e| format!("proxy_on_context_create: {e}"))
                })?;
            }
            let f = vm
                .instance
                .get_typed_func::<(i32, i32, i32), i32>(&mut vm.store, hook)
                .map_err(|e| format!("guest missing {hook}: {e}"))?;
            let action = run_guest_boundary(|| {
                f.call(&mut vm.store, (ctx_id, arg2, arg3))
                    .map_err(|e| format!("{hook} trapped: {e}"))
            })?;
            Ok(action)
        })();
        record_wasm_hook_duration(
            &self.entry.metrics_label,
            phase_label,
            Instant::now().saturating_duration_since(hook_start).as_secs_f64() * 1000.0,
        );
        match result {
            Ok(action) => Ok((ctx_id, action)),
            Err(e) => {
                record_wasm_trap(&self.entry.metrics_label, phase_label);
                self.poison();
                Err(e)
            }
        }
    }

    /// Access host state for snapshot fill / result drain.
    pub fn state_mut(&mut self) -> &mut HostState {
        self.vm_mut().store.data_mut()
    }

    pub fn state(&self) -> &HostState {
        self.vm.as_ref().expect("lease taken").store.data()
    }
}

impl Drop for VmLease {
    fn drop(&mut self) {
        if let Some(vm) = self.vm.take() {
            if self.healthy && self.open_context.is_none() {
                let pool_len = {
                    let mut pool = self.entry.pool.lock();
                    if pool.len() < self.entry.pool_cap {
                        pool.push(vm);
                        pool.len()
                    } else {
                        // Over-cap: drop `vm`, freeing its pooling-allocator slot.
                        drop(vm);
                        record_wasm_instance_discard(&self.entry.metrics_label, "over_capacity");
                        pool.len()
                    }
                };
                set_wasm_pool_size(&self.entry.metrics_label, pool_len);
            } else {
                // A trapped lease and a healthy lease abandoned with an open
                // request context are both unsafe to reuse. Drop the Store
                // without invoking guest code or returning it to the pool.
                record_wasm_instance_discard(&self.entry.metrics_label, "poisoned");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::http::wasm::host_abi::WasmPhase;
    use edgion_resources::plugin_configs::WasmRawConfig;
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

    /// Load an entry for synchronous lease tests. Tick tests retain a runtime owner.
    async fn load(
        mgr: &WasmVmManager,
        config: &edgion_resources::plugin_configs::WasmConfig,
    ) -> Result<Arc<ModuleEntry>, String> {
        mgr.new_runtime(config, CalloutGrant::none()).get_or_load().await
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
            let runtime = manager.new_runtime(&config, CalloutGrant::none());
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
    }

    #[tokio::test]
    async fn runtime_host_panics_record_phase_traps_discard_and_recover() {
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
            let runtime = manager.new_runtime(&config, CalloutGrant::none());
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
    }

    /// Regression (contract-01): two plugins sharing one module source but
    /// carrying different `pluginConfig` must NOT alias the same cached
    /// `ModuleEntry`. Before the fix both computed the source-only key and the
    /// second received the first's configured entry (cross-route isolation
    /// failure).
    #[tokio::test]
    async fn same_source_distinct_plugin_config_do_not_alias() {
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
    }

    /// Regression (contract-01): reloading a plugin with only `pluginConfig`
    /// changed lands on a new configured entry, so a configuration-only hot
    /// reload is not silently a no-op.
    #[tokio::test]
    async fn plugin_config_only_reload_lands_new_entry() {
        let f = write_temp_wasm(MINI_GUEST);
        let mut cfg = file_config(f.path());
        cfg.plugin_config = Some(WasmRawConfig::string("v1"));
        let mgr = WasmVmManager::new_for_test();
        let runtime = mgr.new_runtime(&cfg, CalloutGrant::none());
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
    }

    #[tokio::test]
    async fn plugin_drop_releases_publication_but_preserves_in_flight_lease() {
        let file = write_temp_wasm(MINI_GUEST);
        let manager = WasmVmManager::new_for_test();
        let runtime = manager.new_runtime(&file_config(file.path()), CalloutGrant::none());
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
    }

    #[tokio::test]
    async fn shared_data_metric_handle_follows_plugin_and_lease_lifetime() {
        let file = write_temp_wasm(MINI_GUEST);
        let manager = WasmVmManager::new_for_test();
        let runtime = manager.new_runtime(&file_config(file.path()), CalloutGrant::none());
        let entry = runtime.get_or_load().await.unwrap();
        let label = entry.metrics_label.clone();
        let lease = entry.acquire(100).unwrap();
        drop(entry);
        drop(runtime);
        assert!(crate::observe::metrics::registry::wasm_shared_data_metric_stats()
            .keys
            .contains(&label));
        drop(lease);
        assert!(!crate::observe::metrics::registry::wasm_shared_data_metric_stats()
            .keys
            .contains(&label));
    }

    #[tokio::test]
    async fn dropped_plugin_rejects_late_load_publication_and_tick_start() {
        let file = write_temp_wasm(TICK_GUEST);
        let manager = WasmVmManager::new_for_test();
        let runtime = manager.new_runtime(&file_config(file.path()), CalloutGrant::none());
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
    }

    #[tokio::test]
    async fn identical_sources_keep_plugin_grants_pools_and_shared_data_independent() {
        let file = write_temp_wasm(MINI_GUEST);
        let config = file_config(file.path());
        let manager = WasmVmManager::new_for_test();
        let grant_a = grant(vec!["https://a.example".to_string()], 100);
        let grant_b = grant(vec!["https://b.example".to_string()], 200);
        let a = manager.new_runtime(&config, grant_a.clone());
        let b = manager.new_runtime(&config, grant_b.clone());
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
    }

    #[tokio::test]
    async fn recreated_plugin_reads_replaced_file_and_owns_new_policy() {
        let file = write_temp_wasm(MINI_GUEST);
        let config = file_config(file.path());
        let manager = WasmVmManager::new_for_test();
        let old = manager.new_runtime(&config, CalloutGrant::none());
        let old_entry = old.get_or_load().await.unwrap();
        std::fs::write(file.path(), wat::parse_str(TRAP_GUEST).unwrap()).unwrap();
        let new = manager.new_runtime(&config, CalloutGrant::none());
        let new_entry = new.get_or_load().await.unwrap();
        let mut lease = new_entry.acquire(100).unwrap();
        lease
            .state_mut()
            .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
        assert!(lease.call_on_request_headers(0).is_err());
        assert!(!Arc::ptr_eq(&old_entry, &new_entry));
    }

    #[tokio::test]
    async fn real_guest_receives_exact_non_utf8_vm_and_plugin_config_bytes() {
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
    }

    #[tokio::test]
    async fn gateway_rejects_invalid_raw_config_without_assuming_controller_validation() {
        let file = write_temp_wasm(MINI_GUEST);
        let mut config = file_config(file.path());
        config.plugin_config = Some(WasmRawConfig::base64("YR=="));

        let error = load(&WasmVmManager::new_for_test(), &config)
            .await
            .expect_err("non-canonical base64 must be rejected at the Gateway boundary");
        assert_eq!(error, "pluginConfig must be canonical padded Standard base64");
    }

    #[tokio::test]
    async fn load_from_file_and_lease() {
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
    }

    #[tokio::test]
    async fn request_body_hook_probe_and_call() {
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
    }

    #[tokio::test]
    async fn pool_reuses_instance() {
        let f = write_temp_wasm(MINI_GUEST);
        let mgr = WasmVmManager::new_for_test();
        let entry = load(&mgr, &file_config(f.path())).await.unwrap();
        {
            let _lease = entry.acquire(100).unwrap();
        } // returned to pool on drop
        assert_eq!(entry.pool_len(), 1);
        let _l2 = entry.acquire(100).unwrap();
        assert_eq!(entry.pool_len(), 0); // popped, not re-created
    }

    #[tokio::test]
    async fn repeated_test_manager_drop_releases_engine_pooling_state() {
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
    }

    #[tokio::test]
    async fn abandoned_open_request_context_is_not_returned_to_pool() {
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
    }

    #[tokio::test]
    async fn closed_request_context_returns_vm_to_pool_for_reuse() {
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
    }

    #[tokio::test]
    async fn close_context_trap_discards_vm() {
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
    }

    #[tokio::test]
    async fn second_open_request_context_poisons_lease() {
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
    }

    #[tokio::test]
    async fn poisoned_lease_not_returned() {
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
    }

    #[tokio::test]
    async fn sha256_mismatch_rejected() {
        let f = write_temp_wasm(MINI_GUEST);
        let mut config = file_config(f.path());
        config.sha256 = Some("0".repeat(64));
        let mgr = WasmVmManager::new_for_test();
        let err = load(&mgr, &config).await.unwrap_err();
        assert!(err.contains("sha256"), "got: {err}");
    }

    /// Regression (contract-04): the per-resource `sha256` pin is enforced even
    /// against a warm cache. Two plugins share one file source but pin different
    /// digests; the mismatching one must fail rather than receive the first
    /// plugin's already-loaded (verified) bytes on a cache hit.
    #[tokio::test]
    async fn sha256_pin_enforced_on_warm_cache_hit() {
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
        let runtime = mgr.new_runtime(&good, CalloutGrant::none());
        let verified = runtime.get_or_load().await.expect("correct pin loads");

        // The mismatching config must still fail even though the bytes are warm.
        let err = load(&mgr, &bad).await.unwrap_err();
        assert!(err.contains("sha256"), "warm cache must not bypass the pin; got: {err}");

        // Digest comparison remains case-insensitive for independent plugins.
        let mut good2 = file_config(f.path());
        good2.sha256 = Some(correct.to_ascii_uppercase());
        let uppercase = load(&mgr, &good2).await.expect("uppercase pin loads");
        assert!(!Arc::ptr_eq(&verified, &uppercase));
    }

    #[tokio::test]
    async fn missing_file_is_load_error() {
        let mut config = edgion_resources::plugin_configs::WasmConfig::default();
        config.source.file = Some("/nonexistent/definitely-missing.wasm".to_string());
        let mgr = WasmVmManager::new_for_test();
        assert!(load(&mgr, &config).await.is_err());
    }

    /// ABI check branch 1: a supported version export loads fine.
    #[tokio::test]
    async fn abi_supported_version_loads() {
        let wat = MINI_GUEST.replace(
            "(func (export \"proxy_on_context_create\") (param i32 i32))",
            "(func (export \"proxy_on_context_create\") (param i32 i32))\n      (func (export \"proxy_abi_version_0_2_1\"))",
        );
        let f = write_temp_wasm(&wat);
        let mgr = WasmVmManager::new_for_test();
        assert!(load(&mgr, &file_config(f.path())).await.is_ok());
    }

    /// An unsupported proxy_abi_version_* export fails with the same bounded
    /// compatibility error used for all untrusted module-shape failures.
    #[tokio::test]
    async fn abi_unsupported_version_rejected() {
        let wat = MINI_GUEST.replace(
            "(func (export \"proxy_on_context_create\") (param i32 i32))",
            "(func (export \"proxy_on_context_create\") (param i32 i32))\n      (func (export \"proxy_abi_version_0_1_0\"))",
        );
        let f = write_temp_wasm(&wat);
        let mgr = WasmVmManager::new_for_test();
        let err = load(&mgr, &file_config(f.path())).await.unwrap_err();
        assert_eq!(err, "wasm compatibility check failed");
    }

    #[tokio::test]
    async fn unknown_proxy_hook_export_is_rejected_before_compilation() {
        let wat = MINI_GUEST.replace(
            "(func (export \"proxy_on_request_headers\") (param i32 i32 i32) (result i32) (i32.const 0))",
            "(func (export \"proxy_on_request_headers\") (param i32 i32 i32) (result i32) (i32.const 0))\n      (func (export \"proxy_on_request_header\") (param i32 i32 i32) (result i32) (i32.const 0))",
        );
        let file = write_temp_wasm(&wat);
        let manager = WasmVmManager::new_for_test();
        let error = load(&manager, &file_config(file.path())).await.unwrap_err();
        assert_eq!(error, "wasm compatibility check failed");
    }

    /// ABI check branch 3: no version export at all loads with a WARN
    /// (hand-written WAT test modules and non-standard guests).
    #[tokio::test]
    async fn abi_missing_version_allowed() {
        let f = write_temp_wasm(MINI_GUEST);
        let mgr = WasmVmManager::new_for_test();
        assert!(load(&mgr, &file_config(f.path())).await.is_ok());
    }

    #[test]
    fn reference_guest_matches_host_compatibility_profile() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../edgion-tests/wasm/artifacts/header_demo.wasm");
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
    }

    /// url fetch: an unresolved auth header secret fails the load (no
    /// silent unauthenticated fetch).
    #[tokio::test]
    async fn url_fetch_unresolved_auth_secret_fails_load() {
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
    }

    #[tokio::test]
    async fn url_fetch_transport_error_does_not_expose_configured_url() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve an unused local port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let configured_url =
            format!("http://alice:password@127.0.0.1:{port}/private/module.wasm?token=repo-secret#fragment");

        let error = fetch_url_bytes(&configured_url, None).await.unwrap_err();
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
        let a = tokio::spawn(async move { fetch_url_bytes_with_permits(&permits_a, &url_a, Some(&retry)).await });
        first_a_seen.notified().await;
        let permits_b = permits.clone();
        let b = tokio::spawn(async move { fetch_url_bytes_with_permits(&permits_b, &url_b, None).await });

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
    }

    #[tokio::test]
    async fn preload_claim_is_single_flight_and_respects_retry_deadline() {
        let manager = WasmVmManager::new_for_test();
        let runtime = manager.new_runtime(&WasmConfig::default(), CalloutGrant::none());
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
        let ok_runtime = manager.new_runtime(&ok, CalloutGrant::none());
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
        let bad_runtime = manager.new_runtime(&bad, CalloutGrant::none());

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
        let runtime = manager.new_runtime(&config, CalloutGrant::none());
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
                        caller.data_mut().pending_callouts.push(host_abi::PendingCallout {
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
                        caller.data_mut().pending_callouts.push(host_abi::PendingCallout {
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
    }

    #[tokio::test]
    async fn tick_host_panic_discards_vm_without_adopting_failed_state() {
        let file = write_temp_wasm(&panic_guest(PanicSite::Tick));
        let config = file_config(file.path());
        let manager = WasmVmManager::new_for_test();
        let runtime = manager.new_runtime(&config, CalloutGrant::none());
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
        assert_eq!(entry.pool_len(), 0, "each panicking tick VM must be discarded");
    }

    #[tokio::test]
    async fn tick_callout_callback_panic_discards_vm_without_adopting_failed_state() {
        let file = write_temp_wasm(TICK_CALLOUT_PANIC_GUEST);
        let config = file_config(file.path());
        let manager = WasmVmManager::new_for_test();
        let should_panic = Arc::new(AtomicBool::new(false));
        let callback_calls = Arc::new(AtomicUsize::new(0));
        let dispatch_calls = Arc::new(AtomicUsize::new(0));
        let runtime = manager.new_runtime(&config, grant(vec!["http://example.invalid".to_string()], 10));
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
        assert_eq!(entry.pool_len(), 0, "each panicking callback VM must be discarded");
    }

    /// Plugin destruction must stop a tick round that is parked inside
    /// a callout. The round holds the strong `Arc` that `Drop`'s abort depends
    /// on, so without an explicit stop the entry (and its shared data) would
    /// survive until the callout's own timeout expires.
    #[tokio::test]
    async fn plugin_drop_stops_a_tick_round_blocked_in_a_callout() {
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
        let runtime = mgr.new_runtime(&config, callout_grant);
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
    }

    /// on_tick fires periodically once the guest sets a period during
    /// configure, and stops when the module entry is dropped.
    #[tokio::test]
    async fn tick_task_drives_guest_on_tick() {
        let f = write_temp_wasm(TICK_GUEST);
        let mgr = WasmVmManager::new_for_test();
        let runtime = mgr.new_runtime(&file_config(f.path()), CalloutGrant::none());
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
        let runtime = mgr.new_runtime(&config, grant(vec![server.uri()], 1000));

        let entry = runtime.get_or_load().await.expect("loads");
        assert!(
            wait_shared_key(&entry, "cb").await,
            "success callback must be delivered to the root context"
        );
        assert!(
            !server.received_requests().await.unwrap().is_empty(),
            "the tick callout must reach the target"
        );
    }

    /// Unreachable target: dispatch is accepted (origin allowlisted) but the
    /// callout fails, so the guest sees the 0-header failure callback.
    #[tokio::test]
    async fn tick_callout_unreachable_delivers_failure_callback() {
        let f = write_temp_wasm(TICK_CALLOUT_GUEST);
        let mut config = file_config(f.path());
        // Port 1 is unreachable; still allowlisted so dispatch succeeds.
        config.plugin_config = Some(WasmRawConfig::string("http://127.0.0.1:1"));
        let mgr = WasmVmManager::new_for_test();
        let runtime = mgr.new_runtime(&config, grant(vec!["http://127.0.0.1:1".to_string()], 300));

        let entry = runtime.get_or_load().await.expect("loads");
        assert!(
            wait_shared_key(&entry, "cf").await,
            "failed callout must deliver the 0-header callback"
        );
    }

    /// Target not in the allowlist: dispatch is rejected in the tick phase
    /// exactly as in the request phase, and no request leaves the host.
    #[tokio::test]
    async fn tick_callout_denied_by_allowlist() {
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
        let runtime = mgr.new_runtime(&config, grant(vec!["http://allowed.example.com".to_string()], 1000));

        let entry = runtime.get_or_load().await.expect("loads");
        assert!(wait_shared_key(&entry, "df").await, "guest must observe the denial");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "no callout may reach a non-allowlisted origin"
        );
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
        assert!(mgr.new_runtime(&config, CalloutGrant::none()).try_get().is_none());
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
        let runtime = manager.new_runtime(&config, CalloutGrant::none());
        assert!(runtime.get_or_load().await.unwrap_err().contains("epoch ticker"));
        runtime.spawn_preload();
        assert!(runtime.try_get().is_none());
        assert!(!runtime.state.slot.preload_in_flight.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn oci_source_loads_and_always_recheck_swaps_on_digest_change() {
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
            "source": { "oci": { "image": format!("{host}/test/wasm:latest") } },
            "timeoutMs": 500
        }))
        .unwrap();

        let mgr = WasmVmManager::new_for_test();
        let runtime = mgr.new_runtime(&config, CalloutGrant::none());
        let entry1 = runtime.get_or_load().await.unwrap();
        // Within the re-check interval: cached entry, no new resolution.
        let entry1b = runtime.get_or_load().await.unwrap();
        assert!(Arc::ptr_eq(&entry1, &entry1b));

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
        let new_runtime = mgr.new_runtime(&if_not_present, CalloutGrant::none());
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
    }

    #[tokio::test]
    async fn oci_always_refresh_is_non_blocking_single_flight_and_keeps_lkg_on_failure() {
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
        let runtime = mgr.new_runtime(&config, CalloutGrant::none());
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
    }

    #[tokio::test]
    async fn oci_refresh_stops_old_tick_and_starts_new_tick() {
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
        let runtime = mgr.new_runtime(&config, CalloutGrant::none());
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
    }

    #[tokio::test]
    async fn oci_source_verifies_top_level_sha256_against_final_bytes() {
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
        let err = mgr
            .new_runtime(&config, CalloutGrant::none())
            .get_or_load()
            .await
            .unwrap_err();
        assert!(err.contains("sha256 mismatch"), "got: {err}");
    }
}
