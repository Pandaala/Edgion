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
    Config, Engine, InstanceAllocationStrategy, InstancePre, Linker, Module, PoolingAllocationConfig, Store,
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
use edgion_common::link_sys_core::outbound_tls::GlobalOutboundTlsSnapshot;
use edgion_common::startup::catch_redacted_panic;
use edgion_resources::plugin_configs::{WasmConfig, WasmSourceSpec};
use edgion_resources::{resources::refs::plugin_fields::BoundPluginEntry, EdgionPlugin};

/// How often the background thread bumps the engine epoch. Guest deadlines
/// are expressed in multiples of this tick.
const EPOCH_TICK_MS: u64 = 10;

/// Shared per-chunk execution budget for standalone and request-bound observers.
pub(super) fn response_body_timeout_ms(timeout_ms: u64) -> u64 {
    (timeout_ms / 10).max(10)
}
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

/// Process bound includes idle pools, live requests, timers and retiring generations.
/// Idle pools can consume admission capacity; this is not a fairness guarantee.
const MAX_PROCESS_INSTANCES: usize = 128;
fn instance_permits() -> &'static Arc<tokio::sync::Semaphore> {
    static PERMITS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    PERMITS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(MAX_PROCESS_INSTANCES)))
}

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
    /// One configured-runtime admission owner, including overlapping OCI generations.
    active_requests: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    active_tick_tasks: AtomicUsize,
}

impl TickCalloutState {
    fn new(grant: CalloutGrant, max_active_requests: usize) -> Self {
        Self {
            grant,
            active_requests: Arc::new(tokio::sync::Semaphore::new(max_active_requests)),
            #[cfg(test)]
            active_tick_tasks: AtomicUsize::new(0),
        }
    }
}

#[cfg(test)]
include!("../../../../tests/support/plugins/http/wasm/vm_manager_test/239_struct_item.rs");
#[cfg(test)]
include!("../../../../tests/support/plugins/http/wasm/vm_manager_test/243_impl_item.rs");
#[cfg(test)]
include!("../../../../tests/support/plugins/http/wasm/vm_manager_test/250_impl_item.rs");

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

    pub fn new_runtime(
        &self,
        entry: &BoundPluginEntry<'_>,
        defaults: &GlobalOutboundTlsSnapshot,
        grant: CalloutGrant,
    ) -> Result<WasmRuntime, String> {
        let proof = super::plugin::WasmPlugin::validate_tls_entry(entry, defaults)?;
        let EdgionPlugin::Wasm(config) = entry.config() else {
            return Err("wasm TLS configuration is invalid".to_string());
        };
        // Identity failures reject publication. Crypto/client failures remain load
        // failures so the existing negative cache and failOpen policy still own them.
        let url_client = proof
            .as_ref()
            .map(|proof| prepare_url_client(config.source.fetch.as_ref(), proof, defaults));
        let mut owned_config = config.clone();
        if url_client.is_some() {
            if let Some(fetch) = owned_config.source.fetch.as_mut() {
                fetch.tls = None;
            }
        }
        let tasks = PluginTaskOwner::new();
        super::oci::cache::register_owner();
        Ok(WasmRuntime {
            state: Arc::new(RuntimeState {
                engine: self.engine.clone(),
                config: owned_config,
                url_client,
                slot: Arc::new(LoadSlot::new(effective_oci_always(config))),
                tick_callouts: Arc::new(TickCalloutState::new(grant, config.max_active_requests)),
                tasks: tasks.handle(),
            }),
            tasks,
        })
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
    url_client: Option<Result<reqwest::Client, String>>,
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
        match Self::load_inner_with_linker(
            engine,
            &self.config,
            self.url_client.as_ref(),
            self.tick_callouts.clone(),
            add_host_abi,
        )
        .await
        {
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
        url_client: Option<&Result<reqwest::Client, String>>,
        tick_callouts: Arc<TickCalloutState>,
    ) -> Result<PreparedModule, String> {
        Self::load_inner_with_linker(engine, config, url_client, tick_callouts, add_production_host_abi).await
    }

    async fn load_inner_with_linker<F>(
        engine: Engine,
        config: &WasmConfig,
        url_client: Option<&Result<reqwest::Client, String>>,
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
                super::oci::cache::ArtifactBytes::from(fetch_bytes(&config.source, url_client).await?),
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
        let prepared = match Self::load_inner(
            engine.clone(),
            &self.config,
            self.url_client.as_ref(),
            self.tick_callouts.clone(),
        )
        .await
        {
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
include!("../../../../tests/support/plugins/http/wasm/vm_manager_test/755_impl_item.rs");

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

async fn fetch_bytes(
    source: &WasmSourceSpec,
    url_client: Option<&Result<reqwest::Client, String>>,
) -> Result<Vec<u8>, String> {
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
        return fetch_url_bytes(url, source.fetch.as_ref(), url_client).await;
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
fn prepare_url_client(
    fetch: Option<&edgion_resources::plugin_configs::WasmFetchSpec>,
    proof: &edgion_common::link_sys_core::outbound_tls::ValidatedLocalTlsSelection<'_>,
    defaults: &GlobalOutboundTlsSnapshot,
) -> Result<reqwest::Client, String> {
    use crate::link_sys::http_client::{
        build_outbound_client_with_tls_selection, OutboundClientConfig, OutboundRedirect,
    };
    use crate::plugins::http::common::outbound::egress::{
        default_block_private, egress_dns_cache_ttl, EgressPolicy, EgressResolver,
    };
    use edgion_common::dns::ResolverSpec;

    let timeout = Duration::from_millis(fetch.map(|f| f.timeout_ms).unwrap_or(URL_FETCH_TIMEOUT_SECS * 1000));
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
    build_outbound_client_with_tls_selection(
        OutboundClientConfig {
            pool_max_idle_per_host: Some(4),
            connect_timeout: Some(Duration::from_secs(5)),
            total_timeout: Some(timeout),
            redirect: OutboundRedirect::Deny,
            disable_proxy: true,
            tls_context: Some("WasmFetch"),
            dns_resolver: Some(egress),
            ..Default::default()
        },
        proof,
        defaults,
    )
    .map_err(|e| format!("wasm fetch client: {e}"))
}

async fn fetch_url_bytes(
    url: &str,
    fetch: Option<&edgion_resources::plugin_configs::WasmFetchSpec>,
    url_client: Option<&Result<reqwest::Client, String>>,
) -> Result<Vec<u8>, String> {
    fetch_url_bytes_with_permits(url_fetch_permits(), url, fetch, url_client).await
}

async fn fetch_url_bytes_with_permits(
    permits: &tokio::sync::Semaphore,
    url: &str,
    fetch: Option<&edgion_resources::plugin_configs::WasmFetchSpec>,
    url_client: Option<&Result<reqwest::Client, String>>,
) -> Result<Vec<u8>, String> {
    let timeout = Duration::from_millis(fetch.map(|f| f.timeout_ms).unwrap_or(URL_FETCH_TIMEOUT_SECS * 1000));
    let client = url_client
        .ok_or_else(|| "wasm fetch client was not prepared".to_string())?
        .as_ref()
        .map_err(Clone::clone)?;

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
                    let (body, truncated) = crate::util::http_body::stream_body_capped(resp, MAX_MODULE_BYTES)
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
    callbacks: super::callbacks::GuestCallbacks,
    next_ctx_id: i32,
    /// Released after the Store, including while retained in an idle pool.
    _instance_permit: tokio::sync::OwnedSemaphorePermit,
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
        let permit = self
            .tick_callouts
            .active_requests
            .clone()
            .try_acquire_owned()
            .map_err(|_| "wasm active request limit reached".to_string())?;
        let mut lease = self.acquire_inner(timeout_ms, true)?;
        lease.request_permit = Some(permit);
        Ok(lease)
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
            request_permit: None,
            return_to_pool: true,
        })
    }

    fn acquire_timer(self: &Arc<Self>) -> Result<VmLease, String> {
        Ok(VmLease {
            entry: self.clone(),
            vm: Some(self.new_vm(self.hook_timeout_ms)?),
            timeout_ms: self.hook_timeout_ms,
            healthy: true,
            open_context: None,
            request_permit: None,
            return_to_pool: false,
        })
    }

    /// Start the module tick task once a guest requested a nonzero period.
    /// Idempotent; requires a tokio runtime (skipped otherwise, e.g. in
    /// sync-only unit tests). The resident timer lease retains its loaded entry;
    /// the separate plugin owner explicitly aborts the task when shutting down.
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
            let mut resident: Option<VmLease> = None;
            loop {
                tokio::time::sleep(Duration::from_millis(period)).await;
                let Some(entry) = weak.upgrade() else { break };
                let mut lease = match resident.take().map(Ok).unwrap_or_else(|| entry.acquire_timer()) {
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
                            if !lease.state().frame.pending_callouts.is_empty() {
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
                                        if outcome.leftover != 0 {
                                            drop(lease);
                                            continue;
                                        }
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
                lease.state_mut().release_phase_payloads();
                resident = Some(lease);
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
        let instance_permit = instance_permits()
            .clone()
            .try_acquire_owned()
            .map_err(|_| "wasm process instance limit reached".to_string())?;
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
        let callbacks = super::callbacks::GuestCallbacks::resolve(instance, &mut store);
        Ok(PooledVm {
            _instance_permit: instance_permit,
            store,
            callbacks,
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
    request_permit: Option<tokio::sync::OwnedSemaphorePermit>,
    return_to_pool: bool,
}

impl VmLease {
    /// Open one HTTP context for an entire request binding. Header EOS is
    /// supplied by the transport/body driver, not assumed from the hook type.
    pub fn open_request(&mut self, num_headers: i32, eos: bool) -> Result<i32, String> {
        if self.open_context.is_some() || !self.healthy {
            self.poison();
            return Err("wasm request context unavailable".into());
        }
        let (id, action) =
            self.open_and_call_with("proxy_on_request_headers", "request_headers", num_headers, eos as i32)?;
        self.open_context = Some(id);
        Ok(action)
    }

    pub fn context_id(&self) -> Option<i32> {
        self.open_context
    }

    /// Invoke a later event on the existing guest object without create/delete.
    pub fn call_bound(&mut self, hook: &'static str, size: i32, eos: bool) -> Result<i32, String> {
        let id = self
            .open_context
            .filter(|_| self.healthy)
            .ok_or_else(|| "wasm request context unavailable".to_string())?;
        // Preserve header/request-body hook metrics. Streaming observation uses
        // only its once-per-request diagnostic, never new per-chunk telemetry.
        let phase = match hook {
            "proxy_on_request_body" => Some("request_body"),
            "proxy_on_response_headers" => Some("response_headers"),
            _ => None,
        };
        let started = phase.map(|_| Instant::now());
        let timeout = if hook == "proxy_on_response_body" {
            response_body_timeout_ms(self.timeout_ms)
        } else {
            self.timeout_ms
        };
        let result = (|| {
            let vm = self.vm_mut();
            vm.store.set_epoch_deadline(timeout / EPOCH_TICK_MS + 2);
            let Some(f) = vm.callbacks.http(hook) else {
                return Ok(0);
            };
            run_guest_boundary(|| {
                f.call(&mut vm.store, (id, size, eos as i32))
                    .map_err(|_| "wasm bound hook trapped".into())
            })
        })();
        if let (Some(phase), Some(started)) = (phase, started) {
            record_wasm_hook_duration(
                &self.entry.metrics_label,
                phase,
                started.elapsed().as_secs_f64() * 1000.0,
            );
            if result.is_err() {
                record_wasm_trap(&self.entry.metrics_label, phase);
            }
        }
        if result.is_err() {
            self.poison();
        }
        result
    }

    pub fn call_bound_trailers(&mut self, count: i32) -> Result<i32, String> {
        let id = self
            .open_context
            .filter(|_| self.healthy)
            .ok_or_else(|| "wasm request context unavailable".to_string())?;
        let timeout = self.timeout_ms;
        let result = (|| {
            let vm = self.vm_mut();
            vm.store.set_epoch_deadline(timeout / EPOCH_TICK_MS + 2);
            let Some(f) = vm.callbacks.response_trailers.clone() else {
                return Ok(0);
            };
            run_guest_boundary(|| {
                f.call(&mut vm.store, (id, count))
                    .map_err(|_| "wasm trailer hook trapped".into())
            })
        })();
        if result.is_err() {
            self.poison();
        }
        result
    }

    /// Mark this lease unhealthy so `Drop` discards the VM instead of
    /// returning it to the pool. Public so callers (the Task-5 plugin) can
    /// poison on their own detected failure modes too.
    pub(crate) fn is_healthy(&self) -> bool {
        self.healthy
    }

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
            if vm.callbacks.http("proxy_on_response_body").is_none() {
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
            if vm.callbacks.http("proxy_on_request_body").is_none() {
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
                .callbacks
                .callout
                .clone()
                .ok_or_else(|| "guest missing proxy_on_http_call_response".to_string())?;
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
            let Some(f) = vm.callbacks.tick.clone() else {
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
        if !self.healthy {
            return Err("wasm context is poisoned".into());
        }
        let timeout_ms = self.timeout_ms;
        let result = (|| -> Result<(), String> {
            let vm = self.vm.as_mut().expect("lease taken");
            vm.store.set_epoch_deadline(timeout_ms / EPOCH_TICK_MS + 2);
            if let Some(f) = vm.callbacks.done.clone() {
                let done =
                    run_guest_boundary(|| f.call(&mut vm.store, ctx_id).map_err(|e| format!("proxy_on_done: {e}")))?;
                if done == 0 {
                    return Err("wasm deferred completion is unsupported".into());
                }
            }
            if let Some(f) = vm.callbacks.delete.clone() {
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
            if let Some(f) = vm.callbacks.create.clone() {
                run_guest_boundary(|| {
                    f.call(&mut vm.store, (ctx_id, ROOT_CONTEXT_ID))
                        .map_err(|e| format!("proxy_on_context_create: {e}"))
                })?;
            }
            let f = vm.callbacks.http(hook).ok_or_else(|| format!("guest missing {hook}"))?;
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
        if let Some(mut vm) = self.vm.take() {
            if self.return_to_pool && self.healthy && self.open_context.is_none() {
                vm.store.data_mut().release_phase_payloads();
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
                if !self.healthy || self.open_context.is_some() {
                    record_wasm_instance_discard(&self.entry.metrics_label, "poisoned");
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/plugins/http/wasm/vm_manager_test.rs"]
pub(crate) mod tests;
