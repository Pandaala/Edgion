//! Global Wasm VM manager: one JIT engine, per-source compiled-module cache,
//! and per-module instance pools leased to requests.
//!
//! Design:
//! - One global `Engine`, built once behind a `OnceLock`, using the pooling
//!   allocator (`InstanceAllocationStrategy::Pooling`) for fast per-request
//!   instantiation and epoch interruption (`epoch_interruption(true)`). A
//!   detached background thread increments the engine epoch every
//!   `EPOCH_TICK_MS`; each lease sets a deadline derived from the config's
//!   `timeout_ms` before running guest code, so a stuck/looping guest traps
//!   instead of hanging a worker thread.
//! - Module cache: `DashMap<cache_key, LoadSlot>` where `LoadSlot` is an
//!   `Arc<tokio::sync::Mutex<LoadState>>`. Concurrent `get_or_load` callers
//!   for the same source share one in-flight compile (they all await the
//!   same per-key async mutex) instead of racing independent compiles. A
//!   load error is cached for `LOAD_RETRY_SECS` so a broken source doesn't
//!   get hammered on every request; after that window it is retried.
//! - Bytecode fetch supports `file` (read relative to the gateway work dir),
//!   `inline` (base64), and `url` (plain `reqwest::get` with a fixed
//!   timeout, 200-only). When `sha256` is configured it is verified before
//!   compilation; a mismatch is a load error (never compiled).
//! - Compilation (`Module::new`) runs on `spawn_blocking` since it is a CPU
//!   bound synchronous call that must not block the async executor.
//! - Per module: a `Linker` gets the proxy-wasm host functions registered
//!   via `host_abi::add_to_linker`, then
//!   `linker.define_unknown_imports_as_traps(&module)` is called *after* —
//!   this must come after `add_to_linker` so real host functions are wired
//!   first and only the imports outside the supported subset become traps
//!   (calling them would panic the guest, not the host). The resulting
//!   `InstancePre` is stored on `ModuleEntry` so new instances are cheap to
//!   spin up.
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

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use wasmtime::{
    Config, Engine, Instance, InstanceAllocationStrategy, InstancePre, Linker, Module, PoolingAllocationConfig, Store,
};

use super::host_abi::{self, HostState};
use edgion_resources::plugin_configs::{WasmConfig, WasmSourceSpec};

/// How often the background thread bumps the engine epoch. Guest deadlines
/// are expressed in multiples of this tick.
const EPOCH_TICK_MS: u64 = 10;
/// How long a load error is cached before a fresh `get_or_load` retries the
/// source instead of returning the cached failure immediately.
const LOAD_RETRY_SECS: u64 = 30;
/// proxy-wasm root context id (id 1 is reserved for the VM-wide context that
/// owns `on_vm_start`/`on_configure`; per-request contexts start above it).
const ROOT_CONTEXT_ID: i32 = 1;
/// Fixed timeout for `url`-sourced bytecode fetches.
const URL_FETCH_TIMEOUT_SECS: u64 = 10;

/// Process-wide `WasmVmManager` singleton. All Wasm plugin instances across
/// the gateway share one engine, module cache, and instance pools.
pub fn global_vm_manager() -> &'static WasmVmManager {
    static GLOBAL: OnceLock<WasmVmManager> = OnceLock::new();
    GLOBAL.get_or_init(WasmVmManager::new)
}

type LoadSlot = Arc<tokio::sync::Mutex<LoadState>>;

enum LoadState {
    NotLoaded,
    Loaded(Arc<ModuleEntry>),
    Failed { at: Instant, error: String },
}

/// Global engine + per-source compiled module cache.
///
/// `engine` is a `Result` rather than a bare `Engine`: pooling-allocator
/// reservation can fail (e.g. a restrictive `RLIMIT_AS`), and the gateway's
/// "never crash" invariant means that failure must degrade every Wasm
/// plugin to its `load`-style `fail_mode` at request time, not panic the
/// process during `OnceLock` init.
pub struct WasmVmManager {
    engine: Result<Engine, String>,
    modules: DashMap<String, LoadSlot>,
}

impl WasmVmManager {
    fn new() -> Self {
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
                    modules: DashMap::new(),
                };
            }
        };
        // Detached epoch ticker: bounds guest execution time. One thread per
        // process (this constructor runs once via OnceLock in production;
        // tests use `new_for_test` and get their own ticker thread, which is
        // fine since threads are cheap and each engine only lives as long as
        // its `WasmVmManager`). Only spawned once the engine itself is known
        // good; a failed engine has nothing for the ticker to advance.
        {
            let engine = engine.clone();
            std::thread::Builder::new()
                .name("wasm-epoch-ticker".to_string())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(EPOCH_TICK_MS));
                    engine.increment_epoch();
                })
                .expect("spawn epoch ticker");
        }
        WasmVmManager {
            engine: Ok(engine),
            modules: DashMap::new(),
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
            modules: DashMap::new(),
        }
    }

    /// Borrow the engine, or clone out the cached init-failure message.
    /// Every internal use of the engine goes through this helper so an init
    /// failure is a normal `Err` at every call site instead of a panic.
    fn engine(&self) -> Result<&Engine, String> {
        self.engine.as_ref().map_err(|e| e.clone())
    }

    /// Fast, non-blocking lookup for sync stages. Returns `None` while the
    /// module has not finished loading (or failed recently) rather than
    /// blocking on the load mutex.
    pub fn try_get(&self, cache_key: &str) -> Option<Arc<ModuleEntry>> {
        let slot = self.modules.get(cache_key)?;
        let guard = slot.try_lock().ok()?;
        match &*guard {
            LoadState::Loaded(entry) => Some(entry.clone()),
            _ => None,
        }
    }

    /// Fire-and-forget warm-up (used at plugin construction). Requires a
    /// running tokio runtime; silently skips otherwise (the first real
    /// request will trigger a lazy load via `get_or_load`).
    pub fn spawn_preload(&'static self, config: &WasmConfig) {
        if let Err(e) = self.engine() {
            tracing::warn!(error = %e, "wasm engine unavailable, skipping preload");
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let config = config.clone();
        tokio::spawn(async move {
            if let Err(e) = self.get_or_load(&config).await {
                tracing::warn!(error = %e, "wasm module preload failed");
            }
        });
    }

    /// Load (or return the cached) compiled module + instance pool for a
    /// config. Concurrent callers for the same `cache_key` share one
    /// in-flight load. Errors are cached for `LOAD_RETRY_SECS` to avoid
    /// hammering a bad source.
    pub async fn get_or_load(&self, config: &WasmConfig) -> Result<Arc<ModuleEntry>, String> {
        self.engine()?;
        let key = config.source.cache_key();
        let slot = self
            .modules
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(LoadState::NotLoaded)))
            .clone();
        let mut guard = slot.lock().await;
        match &*guard {
            LoadState::Loaded(entry) => return Ok(entry.clone()),
            LoadState::Failed { at, error } if at.elapsed() < Duration::from_secs(LOAD_RETRY_SECS) => {
                return Err(error.clone());
            }
            _ => {}
        }
        match self.load_inner(config).await {
            Ok(entry) => {
                *guard = LoadState::Loaded(entry.clone());
                Ok(entry)
            }
            Err(e) => {
                tracing::warn!(error = %e, "wasm module load failed");
                *guard = LoadState::Failed {
                    at: Instant::now(),
                    error: e.clone(),
                };
                Err(e)
            }
        }
    }

    async fn load_inner(&self, config: &WasmConfig) -> Result<Arc<ModuleEntry>, String> {
        let engine = self.engine()?.clone();
        let bytes = fetch_bytes(&config.source).await?;
        if let Some(expected) = &config.sha256 {
            let actual = hex::encode(Sha256::digest(&bytes));
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(format!("sha256 mismatch: expected {expected}, got {actual}"));
            }
        }
        let compile_engine = engine.clone();
        let module = tokio::task::spawn_blocking(move || Module::new(&compile_engine, &bytes))
            .await
            .map_err(|e| format!("compile task join error: {e}"))?
            .map_err(|e| format!("wasm compile error: {e}"))?;
        let mut linker: Linker<HostState> = Linker::new(&engine);
        host_abi::add_to_linker(&mut linker).map_err(|e| format!("linker setup: {e}"))?;
        // Must run AFTER add_to_linker: only imports outside the supported
        // proxy-wasm subset become trap stubs; real host functions win.
        linker
            .define_unknown_imports_as_traps(&module)
            .map_err(|e| format!("linker traps: {e}"))?;
        let instance_pre = linker
            .instantiate_pre(&module)
            .map_err(|e| format!("instantiate_pre: {e}"))?;
        Ok(Arc::new(ModuleEntry {
            engine,
            instance_pre,
            pool: Mutex::new(Vec::new()),
            pool_cap: config.instance_pool_size,
            vm_config: config.vm_config.clone().unwrap_or_default().into_bytes(),
            plugin_config: config.plugin_config.clone().unwrap_or_default().into_bytes(),
        }))
    }
}

async fn fetch_bytes(source: &WasmSourceSpec) -> Result<Vec<u8>, String> {
    if let Some(path) = &source.file {
        // Relative paths resolve against the gateway work dir, same as other
        // file-backed configs (access log LocalFile fallback, etc.).
        let resolved = edgion_resources::work_dir().resolve(path);
        return tokio::fs::read(&resolved)
            .await
            .map_err(|e| format!("read wasm file '{}': {e}", resolved.display()));
    }
    if let Some(b64) = &source.inline {
        use base64::Engine as _;
        return base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("decode inline wasm: {e}"));
    }
    if let Some(url) = &source.url {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(URL_FETCH_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("fetch wasm url '{url}': {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("fetch wasm url '{url}': status {}", resp.status()));
        }
        return resp
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| format!("read wasm body: {e}"));
    }
    Err("wasm source has no file/inline/url".to_string())
}

/// A compiled module plus its instance pool. Cheaply shared (`Arc`) across
/// concurrent requests once loaded.
pub struct ModuleEntry {
    engine: Engine,
    instance_pre: InstancePre<HostState>,
    pool: Mutex<Vec<PooledVm>>,
    pool_cap: usize,
    vm_config: Vec<u8>,
    plugin_config: Vec<u8>,
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
        let vm = { self.pool.lock().pop() };
        let vm = match vm {
            Some(vm) => vm,
            None => self.new_vm(timeout_ms)?,
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
        })
    }

    fn new_vm(&self, timeout_ms: u64) -> Result<PooledVm, String> {
        let mut store = Store::new(
            &self.engine,
            HostState::new(self.vm_config.clone(), self.plugin_config.clone()),
        );
        store.set_epoch_deadline(timeout_ms / EPOCH_TICK_MS + 2);
        let instance = self
            .instance_pre
            .instantiate(&mut store)
            .map_err(|e| format!("instantiate: {e}"))?;
        // Optional module initializers (WASI-style reactors/commands). Not
        // every guest exports one; absence is not an error.
        for init in ["_initialize", "_start"] {
            if let Ok(f) = instance.get_typed_func::<(), ()>(&mut store, init) {
                f.call(&mut store, ()).map_err(|e| format!("{init}: {e}"))?;
                break;
            }
        }
        // proxy-wasm lifecycle: root context -> vm_start -> configure.
        if let Ok(f) = instance.get_typed_func::<(i32, i32), ()>(&mut store, "proxy_on_context_create") {
            f.call(&mut store, (ROOT_CONTEXT_ID, 0))
                .map_err(|e| format!("proxy_on_context_create(root): {e}"))?;
        }
        if let Ok(f) = instance.get_typed_func::<(i32, i32), i32>(&mut store, "proxy_on_vm_start") {
            let ok = f
                .call(&mut store, (ROOT_CONTEXT_ID, self.vm_config.len() as i32))
                .map_err(|e| format!("proxy_on_vm_start: {e}"))?;
            if ok == 0 {
                return Err("proxy_on_vm_start returned false".to_string());
            }
        }
        if let Ok(f) = instance.get_typed_func::<(i32, i32), i32>(&mut store, "proxy_on_configure") {
            let ok = f
                .call(&mut store, (ROOT_CONTEXT_ID, self.plugin_config.len() as i32))
                .map_err(|e| format!("proxy_on_configure: {e}"))?;
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

    /// Run the full request-headers hook sequence for a fresh context. Any
    /// trap poisons the lease. Returns the guest Action (0=Continue).
    pub fn call_on_request_headers(&mut self, num_headers: i32) -> Result<i32, String> {
        self.call_headers_hook("proxy_on_request_headers", num_headers)
    }

    /// Same for response headers.
    pub fn call_on_response_headers(&mut self, num_headers: i32) -> Result<i32, String> {
        self.call_headers_hook("proxy_on_response_headers", num_headers)
    }

    fn call_headers_hook(&mut self, hook: &str, num_headers: i32) -> Result<i32, String> {
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
                f.call(&mut vm.store, (ctx_id, ROOT_CONTEXT_ID))
                    .map_err(|e| format!("proxy_on_context_create: {e}"))?;
            }
            let f = vm
                .instance
                .get_typed_func::<(i32, i32, i32), i32>(&mut vm.store, hook)
                .map_err(|e| format!("guest missing {hook}: {e}"))?;
            let action = f
                .call(&mut vm.store, (ctx_id, num_headers, 1))
                .map_err(|e| format!("{hook} trapped: {e}"))?;
            // Close the context (best-effort; failures poison).
            if let Ok(f) = vm.instance.get_typed_func::<i32, i32>(&mut vm.store, "proxy_on_done") {
                f.call(&mut vm.store, ctx_id)
                    .map_err(|e| format!("proxy_on_done: {e}"))?;
            }
            if let Ok(f) = vm.instance.get_typed_func::<i32, ()>(&mut vm.store, "proxy_on_delete") {
                f.call(&mut vm.store, ctx_id)
                    .map_err(|e| format!("proxy_on_delete: {e}"))?;
            }
            Ok(action)
        })();
        if result.is_err() {
            self.poison();
        }
        result
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
            if self.healthy {
                let mut pool = self.entry.pool.lock();
                if pool.len() < self.entry.pool_cap {
                    pool.push(vm);
                }
                // Over-cap: drop `vm` here, freeing its pooling-allocator slot.
            }
            // Unhealthy: drop the Store (frees pooled slots) without reuse.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::http::wasm::host_abi::WasmPhase;
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

    fn file_config(path: &std::path::Path) -> edgion_resources::plugin_configs::WasmConfig {
        let mut c = edgion_resources::plugin_configs::WasmConfig::default();
        c.source.file = Some(path.to_string_lossy().into_owned());
        c
    }

    #[tokio::test]
    async fn load_from_file_and_lease() {
        let f = write_temp_wasm(MINI_GUEST);
        let mgr = WasmVmManager::new_for_test();
        let entry = mgr.get_or_load(&file_config(f.path())).await.expect("loads");
        let mut lease = entry.acquire(100).expect("lease");
        // Instance is initialized: request-headers hook is callable.
        lease
            .state_mut()
            .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
        let action = lease.call_on_request_headers(0).expect("call ok");
        assert_eq!(action, 0);
    }

    #[tokio::test]
    async fn pool_reuses_instance() {
        let f = write_temp_wasm(MINI_GUEST);
        let mgr = WasmVmManager::new_for_test();
        let entry = mgr.get_or_load(&file_config(f.path())).await.unwrap();
        {
            let _lease = entry.acquire(100).unwrap();
        } // returned to pool on drop
        assert_eq!(entry.pool_len(), 1);
        let _l2 = entry.acquire(100).unwrap();
        assert_eq!(entry.pool_len(), 0); // popped, not re-created
    }

    #[tokio::test]
    async fn poisoned_lease_not_returned() {
        let f = write_temp_wasm(TRAP_GUEST);
        let mgr = WasmVmManager::new_for_test();
        let entry = mgr.get_or_load(&file_config(f.path())).await.unwrap();
        {
            let mut lease = entry.acquire(100).unwrap();
            lease
                .state_mut()
                .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
            let res = lease.call_on_request_headers(0);
            assert!(res.is_err(), "trap must surface as error");
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
        let entry = mgr.get_or_load(&file_config(f.path())).await.unwrap();
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
        let err = mgr.get_or_load(&config).await.unwrap_err();
        assert!(err.contains("sha256"), "got: {err}");
    }

    #[tokio::test]
    async fn missing_file_is_load_error() {
        let mut config = edgion_resources::plugin_configs::WasmConfig::default();
        config.source.file = Some("/nonexistent/definitely-missing.wasm".to_string());
        let mgr = WasmVmManager::new_for_test();
        assert!(mgr.get_or_load(&config).await.is_err());
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

        let err = mgr.get_or_load(&config).await.unwrap_err();
        assert!(err.contains("wasm engine init failed"), "got: {err}");

        // The module never gets far enough to populate the cache, so a
        // sync-path caller sees a plain miss rather than a panic too.
        assert!(mgr.try_get(&config.source.cache_key()).is_none());
    }
}
