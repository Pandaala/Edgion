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

use super::host_abi::{self, HostState, WasmPhase};
use super::shared_data::SharedData;
use crate::observe::metrics::registry::{
    record_wasm_compile_duration, record_wasm_hook_duration, record_wasm_instance_discard,
    record_wasm_instance_instantiate, record_wasm_module_evicted, record_wasm_module_load, record_wasm_trap,
    set_wasm_modules_loaded, set_wasm_pool_size,
};
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
/// Fixed timeout for `url`-sourced bytecode fetches when the config carries
/// no `source.fetch.timeoutMs` override.
const URL_FETCH_TIMEOUT_SECS: u64 = 10;
/// How long a cache entry whose plugin refcount dropped to zero is kept
/// before eviction. The grace absorbs reload thrash (full_set replaces every
/// plugin instance, briefly dropping every refcount to zero).
const EVICT_GRACE_SECS: u64 = 60;
/// proxy-wasm ABI version exports the host accepts.
const SUPPORTED_ABI_EXPORTS: [&str; 2] = ["proxy_abi_version_0_2_0", "proxy_abi_version_0_2_1"];
/// Floor for guest-requested tick periods (busy-loop protection).
const MIN_TICK_PERIOD_MS: u64 = 100;

/// Cache key actually used at runtime: the schema-level `cache_key()` plus,
/// for `file` sources, the file's current size+mtime. A same-path overwrite
/// followed by a plugin reload therefore lands on a NEW cache entry (the old
/// one is refcount-evicted), fixing the "replacing the .wasm on disk does not
/// take effect" limitation. A stat failure falls back to the plain key — the
/// subsequent load will fail with a clear read error anyway.
pub fn runtime_cache_key(config: &WasmConfig) -> String {
    let base = config.source.cache_key();
    let Some(path) = &config.source.file else {
        return base;
    };
    let resolved = edgion_resources::work_dir().resolve(path);
    match std::fs::metadata(&resolved) {
        Ok(md) => {
            let mtime_nanos = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("{base};stat={}:{}", md.len(), mtime_nanos)
        }
        Err(_) => base,
    }
}

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
    /// Module cache + plugin refcounts. Behind one `Arc` so the delayed
    /// eviction task can hold the maps without a `'static` borrow of the
    /// manager (test managers are not `'static`).
    cache: Arc<CacheInner>,
    /// Grace period between a refcount hitting zero and the entry being
    /// evicted. Constant in production; overridable in tests.
    evict_grace: Duration,
}

struct CacheInner {
    modules: DashMap<String, LoadSlot>,
    /// cache_key → number of live `WasmPlugin` instances built on it.
    refs: DashMap<String, usize>,
}

impl CacheInner {
    fn new() -> Arc<Self> {
        Arc::new(CacheInner {
            modules: DashMap::new(),
            refs: DashMap::new(),
        })
    }
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
                    cache: CacheInner::new(),
                    evict_grace: Duration::from_secs(EVICT_GRACE_SECS),
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
            cache: CacheInner::new(),
            evict_grace: Duration::from_secs(EVICT_GRACE_SECS),
        }
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self::new()
    }

    /// Test-only: shrink the eviction grace so eviction tests do not sleep
    /// for the production 60s window.
    #[cfg(test)]
    pub fn set_evict_grace_for_test(&mut self, grace: Duration) {
        self.evict_grace = grace;
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
            cache: CacheInner::new(),
            evict_grace: Duration::from_secs(EVICT_GRACE_SECS),
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
        let slot = self.cache.modules.get(cache_key)?;
        let guard = slot.try_lock().ok()?;
        match &*guard {
            LoadState::Loaded(entry) => Some(entry.clone()),
            _ => None,
        }
    }

    /// Register one live `WasmPlugin` built on `cache_key`. Paired with
    /// `unregister_plugin` in the plugin's `Drop`; the count keeps the cache
    /// entry alive across reloads that still reference the same source.
    pub fn register_plugin(&self, cache_key: &str) {
        *self.cache.refs.entry(cache_key.to_string()).or_insert(0) += 1;
    }

    /// Drop one plugin reference. When the count reaches zero the entry is
    /// evicted after `evict_grace` unless a new plugin re-registers the key
    /// in the meantime (reload thrash). In-flight `VmLease`s stay safe: they
    /// hold `Arc<ModuleEntry>` strong refs, so removal from the map only
    /// unpins the entry — memory is freed once the last lease returns.
    pub fn unregister_plugin(&self, cache_key: &str, metrics_label: &str) {
        let now_zero = match self.cache.refs.get_mut(cache_key) {
            Some(mut count) => {
                *count = count.saturating_sub(1);
                *count == 0
            }
            None => false,
        };
        if !now_zero {
            return;
        }
        let cache = self.cache.clone();
        let key = cache_key.to_string();
        let label = metrics_label.to_string();
        let evict = move || {
            // Re-check: a concurrent register_plugin during the grace period
            // keeps the entry. `remove_if` is atomic per entry.
            if cache.refs.remove_if(&key, |_, count| *count == 0).is_some() && cache.modules.remove(&key).is_some() {
                record_wasm_module_evicted(&label);
                set_wasm_modules_loaded(cache.modules.len());
                tracing::info!(module = %label, "wasm module cache entry evicted (refcount zero)");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                let grace = self.evict_grace;
                handle.spawn(async move {
                    tokio::time::sleep(grace).await;
                    evict();
                });
            }
            // No runtime (process teardown): evict immediately. The grace is
            // an anti-thrash optimization, not a correctness requirement.
            Err(_) => evict(),
        }
    }

    /// Fire-and-forget warm-up (used at plugin construction). Requires a
    /// running tokio runtime; silently skips otherwise (the first real
    /// request will trigger a lazy load via `get_or_load`).
    pub fn spawn_preload(&'static self, config: &WasmConfig, cache_key: String) {
        if let Err(e) = self.engine() {
            tracing::warn!(error = %e, "wasm engine unavailable, skipping preload");
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let config = config.clone();
        tokio::spawn(async move {
            if let Err(e) = self.get_or_load(&config, &cache_key).await {
                tracing::warn!(error = %e, "wasm module preload failed");
            }
        });
    }

    /// Load (or return the cached) compiled module + instance pool for a
    /// config. `cache_key` must be the plugin's stored `runtime_cache_key`
    /// (recomputing it here could diverge from the key the plugin registered
    /// and uses on the sync `try_get` path if the source file changed on
    /// disk in between). Concurrent callers for the same key share one
    /// in-flight load. Errors are cached for `LOAD_RETRY_SECS` to avoid
    /// hammering a bad source.
    pub async fn get_or_load(&self, config: &WasmConfig, cache_key: &str) -> Result<Arc<ModuleEntry>, String> {
        self.engine()?;
        let slot = self
            .cache
            .modules
            .entry(cache_key.to_string())
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
        let label = config.source.metrics_label();
        match self.load_inner(config).await {
            Ok(entry) => {
                record_wasm_module_load(&label, "ok");
                *guard = LoadState::Loaded(entry.clone());
                set_wasm_modules_loaded(self.cache.modules.len());
                Ok(entry)
            }
            Err(e) => {
                record_wasm_module_load(&label, "error");
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
        let label = config.source.metrics_label();
        let compile_engine = engine.clone();
        let compile_start = Instant::now();
        let module = tokio::task::spawn_blocking(move || Module::new(&compile_engine, &bytes))
            .await
            .map_err(|e| format!("compile task join error: {e}"))?
            .map_err(|e| format!("wasm compile error: {e}"))?;
        record_wasm_compile_duration(&label, compile_start.elapsed().as_secs_f64() * 1000.0);
        check_abi_version(&module)?;
        // Probe the body hook once at load so the per-chunk body filter can
        // skip leasing an instance for header-only guests (see
        // `has_body_hook`); export lookup on every chunk would otherwise
        // contend the pool mutex for no benefit on large streamed responses.
        let has_body_hook = module.exports().any(|e| e.name() == "proxy_on_response_body");
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
        let entry = Arc::new(ModuleEntry {
            engine,
            instance_pre,
            pool: Mutex::new(Vec::new()),
            pool_cap: config.instance_pool_size,
            vm_config: config.vm_config.clone().unwrap_or_default().into_bytes(),
            plugin_config: config.plugin_config.clone().unwrap_or_default().into_bytes(),
            shared_data: Arc::new(SharedData::new(label.clone())),
            metrics_label: label,
            hook_timeout_ms: config.timeout_ms,
            tick_task: Mutex::new(None),
            has_body_hook,
            body_pause_warned: std::sync::atomic::AtomicBool::new(false),
        });
        // Eagerly warm one instance: validates guest init at load time (an
        // init failure becomes a clear, negative-cached load error instead
        // of a per-request acquire failure) and lets a tick period the guest
        // set during on_configure start the tick task even before the first
        // request arrives.
        let warm = entry
            .acquire(entry.hook_timeout_ms)
            .map_err(|e| format!("wasm instance init failed: {e}"))?;
        drop(warm);
        Ok(entry)
    }
}

/// Enforce the proxy-wasm ABI version marker exported by the guest.
///
/// - A supported marker (`proxy_abi_version_0_2_0` / `_0_2_1`) passes.
/// - Any other `proxy_abi_version_*` export is a load error: the guest
///   targets an ABI the host does not implement, and failing the load with a
///   clear message beats the runtime "unknown import trapped" fallback.
/// - No marker at all passes with a WARN: hand-written test modules and
///   non-standard guests do not export one, and the defined-as-trap fallback
///   still bounds whatever they call.
fn check_abi_version(module: &Module) -> Result<(), String> {
    let versions: Vec<&str> = module
        .exports()
        .map(|e| e.name())
        .filter(|n| n.starts_with("proxy_abi_version_"))
        .collect();
    if versions.is_empty() {
        tracing::warn!("wasm module exports no proxy_abi_version_* marker; loading anyway (non-standard guest)");
        return Ok(());
    }
    if versions.iter().any(|v| SUPPORTED_ABI_EXPORTS.contains(v)) {
        return Ok(());
    }
    Err(format!(
        "unsupported proxy-wasm ABI version ({}); supported: proxy_abi_version_0_2_0, proxy_abi_version_0_2_1",
        versions.join(", ")
    ))
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

/// Bound on concurrently in-flight url fetches across all modules. Same-url
/// callers already share one in-flight load via the per-key `LoadSlot`
/// mutex; this caps distinct-url fan-out during a startup storm.
fn url_fetch_permits() -> &'static tokio::sync::Semaphore {
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

    let _permit = url_fetch_permits()
        .acquire()
        .await
        .map_err(|_| "wasm fetch semaphore closed".to_string())?;

    let mut last_error = String::new();
    for attempt in 0..attempts {
        if attempt > 0 {
            let delay = backoff_ms.saturating_mul(1u64 << (attempt - 1).min(10));
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let mut req = client.get(url).timeout(timeout);
        if let Some(auth) = &auth_header {
            req = req.header(reqwest::header::AUTHORIZATION, auth.clone());
        }
        match req.send().await {
            Err(e) => {
                last_error = format!("fetch wasm url '{url}': {e}");
            }
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return resp
                        .bytes()
                        .await
                        .map(|b| b.to_vec())
                        .map_err(|e| format!("read wasm body: {e}"));
                }
                last_error = format!("fetch wasm url '{url}': status {status}");
                if status.is_client_error() {
                    // 4xx is definitive — do not retry.
                    return Err(last_error);
                }
            }
        }
    }
    Err(last_error)
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
    /// Human-readable source descriptor for metrics labels.
    metrics_label: String,
    /// Module-wide shared KV, cloned into every pooled VM's HostState.
    /// Freed with this entry (i.e. follows cache eviction).
    shared_data: Arc<SharedData>,
    /// Per-hook budget for background (tick) leases; mirrors the config's
    /// `timeoutMs`.
    hook_timeout_ms: u64,
    /// Background `proxy_on_tick` driver, started once when a guest sets a
    /// nonzero tick period. Holds a `Weak` back-reference; aborted on Drop.
    tick_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Whether the guest exports `proxy_on_response_body`. Computed once
    /// at load; lets the body-filter stage skip the per-chunk lease entirely
    /// for header-only guests.
    has_body_hook: bool,
    /// Latched once the guest is first seen returning `Action::Pause` from the
    /// body hook. The incompatibility is a property of the module (a guest
    /// either uses the standard buffering idiom or it does not), so the WARN is
    /// emitted once per module rather than once per chunk, which would flood.
    body_pause_warned: std::sync::atomic::AtomicBool,
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
                self.maybe_start_tick(period);
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
        })
    }

    /// Start the module tick task once a guest requested a nonzero period.
    /// Idempotent; requires a tokio runtime (skipped otherwise, e.g. in
    /// sync-only unit tests). The task holds only a `Weak` back-reference,
    /// so cache eviction still drops the entry — `Drop` aborts the task.
    fn maybe_start_tick(self: &Arc<Self>, period_ms: u64) {
        if period_ms == 0 || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut guard = self.tick_task.lock();
        if guard.is_some() {
            return;
        }
        let weak = Arc::downgrade(self);
        let label = self.metrics_label.clone();
        let handle = tokio::spawn(async move {
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
                // Fresh phase state; dispatch is NOT armed, so callouts from
                // on_tick observe UNIMPLEMENTED (documented limitation).
                lease.state_mut().begin_phase(
                    WasmPhase::Tick,
                    Vec::new(),
                    Vec::new(),
                    std::collections::HashMap::new(),
                );
                if let Err(e) = lease.call_on_tick() {
                    tracing::warn!(error = %e, module = %label, "wasm on_tick trapped");
                }
                // The guest may adjust (or stop) its own tick period.
                let next = lease.state().tick_period_ms;
                drop(lease);
                if next == 0 {
                    tracing::info!(module = %label, "wasm tick stopped by guest (period 0)");
                    break;
                }
                period = next.max(MIN_TICK_PERIOD_MS);
            }
        });
        *guard = Some(handle);
    }

    /// Whether the guest exports `proxy_on_response_body`. The body
    /// filter stage checks this after the cheap `try_get` lookup and returns
    /// early for header-only guests, avoiding a per-chunk pool lease.
    pub fn has_body_hook(&self) -> bool {
        self.has_body_hook
    }

    /// Human-readable module descriptor (metrics label).
    pub fn metrics_label(&self) -> &str {
        &self.metrics_label
    }

    /// Latch the body-Pause warning: returns `true` exactly once per module,
    /// so the caller emits a single WARN instead of one per body chunk.
    pub fn latch_body_pause_warning(&self) -> bool {
        !self.body_pause_warned.swap(true, std::sync::atomic::Ordering::Relaxed)
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

    /// Open a fresh context and run the request-headers hook WITHOUT closing
    /// the context, so a paused guest can later receive
    /// `proxy_on_http_call_response` on it. The caller must eventually call
    /// `close_context` (any outcome, including local response).
    pub fn call_on_request_headers_open(&mut self, num_headers: i32) -> Result<(i32, i32), String> {
        self.open_and_call("proxy_on_request_headers", "request_headers", num_headers)
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
            f.call(&mut vm.store, (ctx_id, token as i32, num_headers, body_size, 0))
                .map_err(|e| format!("proxy_on_http_call_response trapped: {e}"))?;
            Ok(())
        })();
        record_wasm_hook_duration(
            &self.entry.metrics_label,
            "http_call_response",
            hook_start.elapsed().as_secs_f64() * 1000.0,
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
            f.call(&mut vm.store, ROOT_CONTEXT_ID)
                .map_err(|e| format!("proxy_on_tick trapped: {e}"))
        })();
        record_wasm_hook_duration(
            &self.entry.metrics_label,
            "tick",
            hook_start.elapsed().as_secs_f64() * 1000.0,
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
                f.call(&mut vm.store, ctx_id)
                    .map_err(|e| format!("proxy_on_done: {e}"))?;
            }
            if let Ok(f) = vm.instance.get_typed_func::<i32, ()>(&mut vm.store, "proxy_on_delete") {
                f.call(&mut vm.store, ctx_id)
                    .map_err(|e| format!("proxy_on_delete: {e}"))?;
            }
            Ok(())
        })();
        if result.is_err() {
            record_wasm_trap(&self.entry.metrics_label, "context_close");
            self.poison();
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
                f.call(&mut vm.store, (ctx_id, ROOT_CONTEXT_ID))
                    .map_err(|e| format!("proxy_on_context_create: {e}"))?;
            }
            let f = vm
                .instance
                .get_typed_func::<(i32, i32, i32), i32>(&mut vm.store, hook)
                .map_err(|e| format!("guest missing {hook}: {e}"))?;
            let action = f
                .call(&mut vm.store, (ctx_id, arg2, arg3))
                .map_err(|e| format!("{hook} trapped: {e}"))?;
            Ok(action)
        })();
        record_wasm_hook_duration(
            &self.entry.metrics_label,
            phase_label,
            hook_start.elapsed().as_secs_f64() * 1000.0,
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
            if self.healthy {
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
                // Unhealthy: drop the Store (frees pooled slots) without reuse.
                record_wasm_instance_discard(&self.entry.metrics_label, "poisoned");
            }
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

    /// Test shorthand: load with the runtime cache key derived from the
    /// config, matching what `WasmPlugin::build` does in production.
    async fn load(
        mgr: &WasmVmManager,
        config: &edgion_resources::plugin_configs::WasmConfig,
    ) -> Result<Arc<ModuleEntry>, String> {
        mgr.get_or_load(config, &runtime_cache_key(config)).await
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
    async fn poisoned_lease_not_returned() {
        let f = write_temp_wasm(TRAP_GUEST);
        let mgr = WasmVmManager::new_for_test();
        let entry = load(&mgr, &file_config(f.path())).await.unwrap();
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

    /// ABI check branch 2: an unsupported proxy_abi_version_* export is a
    /// load error with a readable message (not a runtime trap).
    #[tokio::test]
    async fn abi_unsupported_version_rejected() {
        let wat = MINI_GUEST.replace(
            "(func (export \"proxy_on_context_create\") (param i32 i32))",
            "(func (export \"proxy_on_context_create\") (param i32 i32))\n      (func (export \"proxy_abi_version_0_1_0\"))",
        );
        let f = write_temp_wasm(&wat);
        let mgr = WasmVmManager::new_for_test();
        let err = load(&mgr, &file_config(f.path())).await.unwrap_err();
        assert!(err.contains("unsupported proxy-wasm ABI version"), "got: {err}");
        assert!(err.contains("proxy_abi_version_0_1_0"), "got: {err}");
    }

    /// ABI check branch 3: no version export at all loads with a WARN
    /// (hand-written WAT test modules and non-standard guests).
    #[tokio::test]
    async fn abi_missing_version_allowed() {
        let f = write_temp_wasm(MINI_GUEST);
        let mgr = WasmVmManager::new_for_test();
        assert!(load(&mgr, &file_config(f.path())).await.is_ok());
    }

    /// Refcount eviction: when the last plugin unregisters a cache key, the
    /// entry disappears after the grace period; an in-flight lease taken
    /// before eviction stays usable (Arc keeps the entry alive).
    #[tokio::test]
    async fn refcount_zero_evicts_entry_after_grace() {
        let f = write_temp_wasm(MINI_GUEST);
        let config = file_config(f.path());
        let key = runtime_cache_key(&config);
        let mut mgr = WasmVmManager::new_for_test();
        mgr.set_evict_grace_for_test(Duration::from_millis(50));

        mgr.register_plugin(&key);
        let entry = load(&mgr, &config).await.expect("loads");
        let mut lease = entry.acquire(100).expect("lease");

        mgr.unregister_plugin(&key, "test-module");
        // Poll rather than a single fixed sleep: the eviction task's timer
        // can be delayed under parallel test load.
        let mut evicted = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if mgr.try_get(&key).is_none() {
                evicted = true;
                break;
            }
        }
        assert!(evicted, "entry must be evicted after grace");

        // The lease taken before eviction is still safe to drive.
        lease
            .state_mut()
            .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
        assert!(lease.call_on_request_headers(0).is_ok());
    }

    /// A re-register during the grace period cancels the eviction.
    #[tokio::test]
    async fn reregister_during_grace_keeps_entry() {
        let f = write_temp_wasm(MINI_GUEST);
        let config = file_config(f.path());
        let key = runtime_cache_key(&config);
        let mut mgr = WasmVmManager::new_for_test();
        mgr.set_evict_grace_for_test(Duration::from_millis(100));

        mgr.register_plugin(&key);
        load(&mgr, &config).await.expect("loads");
        mgr.unregister_plugin(&key, "test-module");
        // Reload thrash: a new plugin instance re-registers within the grace.
        mgr.register_plugin(&key);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            mgr.try_get(&key).is_some(),
            "re-registered entry must survive the grace window"
        );
    }

    /// File-source cache keys are content-aware: overwriting the same path
    /// with different bytes yields a different runtime cache key.
    #[test]
    fn runtime_cache_key_changes_when_file_replaced() {
        let f = write_temp_wasm(MINI_GUEST);
        let config = file_config(f.path());
        let key_before = runtime_cache_key(&config);
        // Overwrite with different content (different size guarantees a
        // stat-visible change even on coarse-mtime filesystems).
        let bytes = wat::parse_str(TRAP_GUEST).unwrap();
        std::fs::write(f.path(), &bytes).unwrap();
        let key_after = runtime_cache_key(&config);
        assert_ne!(key_before, key_after, "same-path overwrite must change the key");
        // Non-file sources are unaffected by stat.
        let mut inline = edgion_resources::plugin_configs::WasmConfig::default();
        inline.source.inline = Some("AAAA".to_string());
        assert_eq!(runtime_cache_key(&inline), inline.source.cache_key());
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
        let err = load(&mgr, &config).await.unwrap_err();
        assert!(err.contains("status 401"), "got: {err}");
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "4xx must not be retried"
        );
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

    /// on_tick fires periodically once the guest sets a period during
    /// configure, and stops when the module entry is dropped.
    #[tokio::test]
    async fn tick_task_drives_guest_on_tick() {
        let f = write_temp_wasm(TICK_GUEST);
        let mgr = WasmVmManager::new_for_test();
        let entry = load(&mgr, &file_config(f.path())).await.expect("loads");
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
        assert!(mgr.try_get(&config.source.cache_key()).is_none());
    }
}
