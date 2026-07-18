//! proxy-wasm ABI v0.2 host bridge (headers + local response subset).
//!
//! Design: host functions never touch the PluginSession. The plugin snapshots
//! request/response headers into `HostState` before invoking the guest, the
//! guest mutates the snapshot (so it can read back its own writes) while ops
//! are also queued, and the plugin drains `pending_ops` onto the session after
//! the guest returns. Buffers returned to the guest are allocated by calling
//! the guest's own `proxy_on_memory_allocate` export.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use wasmtime::{Caller, Extern, Linker};

pub const MAP_HTTP_REQUEST_HEADERS: i32 = 0;
pub const MAP_HTTP_RESPONSE_HEADERS: i32 = 2;
pub const MAP_HTTP_CALL_RESPONSE_HEADERS: i32 = 6;

pub const BUF_HTTP_RESPONSE_BODY: i32 = 1;
pub const BUF_HTTP_CALL_RESPONSE_BODY: i32 = 4;
pub const BUF_VM_CONFIGURATION: i32 = 6;
pub const BUF_PLUGIN_CONFIGURATION: i32 = 7;

pub const STATUS_OK: i32 = 0;
pub const STATUS_NOT_FOUND: i32 = 1;
pub const STATUS_BAD_ARGUMENT: i32 = 2;
pub const STATUS_INVALID_MEMORY_ACCESS: i32 = 6;
pub const STATUS_CAS_MISMATCH: i32 = 8;
pub const STATUS_INTERNAL_FAILURE: i32 = 10;
pub const STATUS_UNIMPLEMENTED: i32 = 12;

/// Max callouts one request context may dispatch (across all rounds).
/// Bounds guest-driven outbound amplification.
pub const MAX_CALLOUTS_PER_REQUEST: u32 = 8;

/// Aggregate per-phase budget for host-retained ABI payload bytes.
///
/// `read_guest` bounds each individual copy to guest linear-memory size, but
/// nothing bounds the SUM a guest retains across many host calls in one hook:
/// a guest can reuse one small buffer and append arbitrarily many owned copies
/// to `pending_ops` / `log_lines` / `properties` until `begin_phase` clears
/// them after the hook. Epoch interruption bounds execution TIME, not
/// allocation. This budget bounds the host-heap peak a single hook can drive.
/// 32 MiB is far above any legitimate header/log/property working set; only a
/// pathological accumulation loop trips it.
pub const MAX_RETAINED_ABI_BYTES: usize = 32 * 1024 * 1024;

/// Aggregate per-phase budget for host-retained ABI operation count. Bounds a
/// flood of tiny (near-zero-byte) operations whose per-item structural
/// overhead the byte budget alone under-charges.
pub const MAX_RETAINED_ABI_OPS: usize = 200_000;

/// Fixed per-operation structural overhead (Vec/String headers, enum
/// discriminant) charged in addition to payload bytes, so many tiny ops are
/// accounted at their real heap footprint. Mirrors `shared_data`'s
/// `ENTRY_OVERHEAD_BYTES`.
const ABI_OP_OVERHEAD_BYTES: usize = 64;

/// Max guest linear-memory size (bytes) enforced by the Store `ResourceLimiter`
/// in `WasmVmManager::new_vm`. The pooling allocator's default per-instance
/// memory cap is effectively the full wasm32 4 GiB, so without this an
/// `(memory 1)` guest could grow arbitrarily and make each `read_guest`
/// transient copy that large. 128 MiB is well above any real proxy-wasm guest
/// working set.
pub const MAX_GUEST_MEMORY_BYTES: usize = 128 * 1024 * 1024;

/// Max guest table entries enforced by the Store `ResourceLimiter`. Bounds
/// indirect-call table growth as defense in depth; legitimate proxy-wasm
/// guests use tiny tables.
pub const MAX_GUEST_TABLE_ELEMENTS: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WasmPhase {
    #[default]
    RequestHeaders,
    ResponseHeaders,
    ResponseBody,
    /// Background `proxy_on_tick` invocation (no request in scope).
    Tick,
}

#[derive(Debug, Clone)]
pub enum HeaderOp {
    Set {
        map: i32,
        name: String,
        value: String,
    },
    Add {
        map: i32,
        name: String,
        value: String,
    },
    Remove {
        map: i32,
        name: String,
    },
    /// Whole-map replacement (`proxy_set_header_map_pairs`). `remove` carries
    /// the deduplicated set of header names present in the live map *before*
    /// the replacement, so `apply_ops` can drop every name the guest omitted
    /// (the replacement must be a true replacement, not an additive set —
    /// otherwise omitted credentials/identity headers would leak downstream).
    ReplaceAll {
        map: i32,
        remove: Vec<String>,
        pairs: Vec<(String, String)>,
    },
}

#[derive(Debug, Clone)]
pub struct LocalResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

/// One guest-dispatched HTTP callout, queued by `proxy_dispatch_http_call`
/// and executed by the plugin's async request path while the guest is
/// paused.
#[derive(Debug, Clone)]
pub struct PendingCallout {
    pub token: u32,
    /// Normalized `scheme://host:port` origin (already allowlist-checked).
    pub origin: String,
    /// `:path` pseudo header (leading `/` guaranteed).
    pub path: String,
    /// `:method` pseudo header.
    pub method: String,
    /// `:authority` pseudo header, sent as the `Host` header when present.
    pub authority: Option<String>,
    /// Non-pseudo request headers.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Guest-supplied timeout (0 = use the config cap).
    pub timeout_ms: u32,
}

/// The callout response currently being delivered to the guest via
/// `proxy_on_http_call_response`. Empty `headers` signals a failed callout
/// (proxy-wasm convention: the callback fires with 0 headers).
#[derive(Debug, Clone, Default)]
pub struct CalloutResponse {
    pub token: u32,
    /// Includes `:status` as the first pair on success; empty on failure.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Per-Store host state. Config fields persist across leases; request-scoped
/// fields are reset by `begin_phase`.
#[derive(Default)]
pub struct HostState {
    pub phase: WasmPhase,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub pending_ops: Vec<HeaderOp>,
    pub local_response: Option<LocalResponse>,
    pub properties: HashMap<String, String>,
    pub vm_config: Vec<u8>,
    pub plugin_config: Vec<u8>,
    /// (level, message) lines emitted via proxy_log during the current phase.
    pub log_lines: Vec<(i32, String)>,
    /// Metrics label of the owning module (set once per Store).
    pub metrics_label: String,

    // ---- Callout state (armed per phase; see `arm_dispatch`) ----
    /// Whether `proxy_dispatch_http_call` is accepted in the current phase.
    pub dispatch_enabled: bool,
    /// Normalized origins the guest may call out to (empty = deny all).
    pub callout_allowlist: Arc<[String]>,
    /// Cap (and 0-fallback) for guest-supplied callout timeouts, ms.
    pub callout_timeout_cap_ms: u64,
    /// Callouts queued by the guest, drained by the plugin's async path.
    pub pending_callouts: Vec<PendingCallout>,
    /// The response being delivered in the current
    /// `proxy_on_http_call_response` invocation.
    pub callout_response: Option<CalloutResponse>,
    /// Set by `proxy_continue_request` / `proxy_continue_stream(request)`.
    pub continue_requested: bool,
    /// Total callouts dispatched in this phase (cap enforcement).
    callouts_dispatched: u32,
    next_callout_token: u32,

    // ---- Body observation state (read-only chunk window) ----
    /// Current response-body chunk exposed via
    /// `get_buffer_bytes(HttpResponseBody)`. Never buffers across chunks.
    pub body_chunk: Vec<u8>,

    /// Guest-requested tick period in ms (0 = no tick). Module-lifetime
    /// state (typically set during `on_configure`), NOT reset per phase;
    /// the module tick task reads it after instance init.
    pub tick_period_ms: u64,

    /// Module-wide shared KV (proxy-wasm shared data). Shared by every
    /// pooled VM of the same `ModuleEntry`; process-local by design.
    pub shared_data: Arc<super::shared_data::SharedData>,

    // ---- Per-phase ABI retention budget (see `charge_abi`) ----
    /// Host-retained ABI payload bytes charged this phase across the
    /// append-only accumulators (`pending_ops`, `log_lines`, `properties`).
    /// Reset by `begin_phase`. Bounds the host-heap peak a single hook can
    /// drive before `begin_phase` clears it.
    retained_bytes: usize,
    /// Host-retained ABI operation count charged this phase. Reset by
    /// `begin_phase`. Complements `retained_bytes` for tiny-op floods.
    retained_ops: usize,
}

impl HostState {
    pub fn new(
        vm_config: Vec<u8>,
        plugin_config: Vec<u8>,
        metrics_label: String,
        shared_data: Arc<super::shared_data::SharedData>,
    ) -> Self {
        HostState {
            vm_config,
            plugin_config,
            metrics_label,
            shared_data,
            ..Default::default()
        }
    }

    /// Reset request-scoped state for a new phase invocation.
    pub fn begin_phase(
        &mut self,
        phase: WasmPhase,
        request_headers: Vec<(String, String)>,
        response_headers: Vec<(String, String)>,
        properties: HashMap<String, String>,
    ) {
        self.phase = phase;
        self.request_headers = request_headers;
        self.response_headers = response_headers;
        self.properties = properties;
        self.pending_ops.clear();
        self.local_response = None;
        self.log_lines.clear();
        self.dispatch_enabled = false;
        self.callout_allowlist = Arc::from(Vec::new());
        self.callout_timeout_cap_ms = 0;
        self.pending_callouts.clear();
        self.callout_response = None;
        self.continue_requested = false;
        self.callouts_dispatched = 0;
        // Tokens start at 1: some proxy-wasm SDKs treat a 0 call-token as an
        // invalid/sentinel value (Envoy issues nonzero tokens).
        self.next_callout_token = 1;
        self.body_chunk.clear();
        self.retained_bytes = 0;
        self.retained_ops = 0;
    }

    /// Charge `payload` bytes plus fixed per-op overhead against the per-phase
    /// ABI retention budget. Returns `Err(STATUS_INTERNAL_FAILURE)` when either
    /// the byte or op budget would be exceeded — WITHOUT mutating any counter —
    /// so the caller rejects before touching the snapshot (no partial
    /// mutation). Commits the charge and returns `Ok(())` otherwise.
    fn charge_abi(&mut self, payload: usize) -> Result<(), i32> {
        let cost = payload.saturating_add(ABI_OP_OVERHEAD_BYTES);
        let new_bytes = self.retained_bytes.saturating_add(cost);
        let new_ops = self.retained_ops.saturating_add(1);
        if new_bytes > MAX_RETAINED_ABI_BYTES || new_ops > MAX_RETAINED_ABI_OPS {
            return Err(STATUS_INTERNAL_FAILURE);
        }
        self.retained_bytes = new_bytes;
        self.retained_ops = new_ops;
        Ok(())
    }

    /// Enable `proxy_dispatch_http_call` for the current phase (request and
    /// tick phases — the sync response stages cannot await a callout).
    pub fn arm_dispatch(&mut self, allowlist: Arc<[String]>, timeout_cap_ms: u64) {
        self.dispatch_enabled = true;
        self.callout_allowlist = allowlist;
        self.callout_timeout_cap_ms = timeout_cap_ms;
    }

    fn map_mut(&mut self, map: i32) -> Option<&mut Vec<(String, String)>> {
        match map {
            MAP_HTTP_REQUEST_HEADERS => Some(&mut self.request_headers),
            MAP_HTTP_RESPONSE_HEADERS => Some(&mut self.response_headers),
            _ => None,
        }
    }

    fn map_ref(&self, map: i32) -> Option<&[(String, String)]> {
        match map {
            MAP_HTTP_REQUEST_HEADERS => Some(&self.request_headers),
            MAP_HTTP_RESPONSE_HEADERS => Some(&self.response_headers),
            // The callout-response map is only populated during a
            // `proxy_on_http_call_response` delivery; elsewhere it reads empty
            // (not an error). Returned as a borrow so a per-header lookup does
            // not clone the whole map.
            MAP_HTTP_CALL_RESPONSE_HEADERS => Some(
                self.callout_response
                    .as_ref()
                    .map(|r| r.headers.as_slice())
                    .unwrap_or(&[]),
            ),
            _ => None,
        }
    }
}

/// Store-level guest resource limiter (installed in
/// `WasmVmManager::new_vm`). Bounds guest linear-memory and table growth as
/// defense in depth: the pooling allocator's default per-instance memory cap
/// is effectively the full wasm32 4 GiB, so this is what keeps a `(memory 1)`
/// guest — and therefore every `read_guest` transient copy sized by that
/// memory — within a sane bound. It does NOT bound host-side accumulation
/// across calls; that is `charge_abi`'s job.
impl wasmtime::ResourceLimiter for HostState {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= MAX_GUEST_MEMORY_BYTES)
    }

    fn table_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= MAX_GUEST_TABLE_ELEMENTS)
    }
}

// ---------- proxy-wasm serialized map codec ----------

pub fn encode_map(pairs: &[(String, String)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + pairs.iter().map(|(k, v)| k.len() + v.len() + 10).sum::<usize>());
    out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
    for (k, v) in pairs {
        out.extend_from_slice(&(k.len() as u32).to_le_bytes());
        out.extend_from_slice(&(v.len() as u32).to_le_bytes());
    }
    for (k, v) in pairs {
        out.extend_from_slice(k.as_bytes());
        out.push(0);
        out.extend_from_slice(v.as_bytes());
        out.push(0);
    }
    out
}

pub fn decode_map(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    if bytes.len() < 4 {
        return None;
    }
    let n = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    // `n` is guest-controlled and unbounded (up to u32::MAX); pre-allocating
    // `Vec::with_capacity(n)` directly on a malformed payload can request
    // tens of GB and abort the whole host process. Cap the pre-allocation by
    // what `bytes` could actually hold: the wire size-table entry is 8 bytes
    // (two u32 LE lengths), hence the `/ 8` below — not to be confused with
    // the 16-byte `(usize, usize)` in-memory element that `sizes` collects
    // those wire entries into on a 64-bit host. The per-iteration
    // `off + 8 > bytes.len()` guard below still rejects an oversized `n`
    // exactly as before for genuinely malformed input.
    let cap = n.min(bytes.len().saturating_sub(4) / 8);
    let mut sizes = Vec::with_capacity(cap);
    let mut off = 4;
    for _ in 0..n {
        if off + 8 > bytes.len() {
            return None;
        }
        let k = u32::from_le_bytes(bytes[off..off + 4].try_into().ok()?) as usize;
        let v = u32::from_le_bytes(bytes[off + 4..off + 8].try_into().ok()?) as usize;
        sizes.push((k, v));
        off += 8;
    }
    let mut pairs = Vec::with_capacity(cap);
    for (k, v) in sizes {
        if off + k + 1 + v + 1 > bytes.len() {
            return None;
        }
        let key = String::from_utf8_lossy(&bytes[off..off + k]).into_owned();
        off += k + 1; // skip NUL
        let val = String::from_utf8_lossy(&bytes[off..off + v]).into_owned();
        off += v + 1; // skip NUL
        pairs.push((key, val));
    }
    Some(pairs)
}

/// Sub-slice of `data` starting at `start` (guest-controlled, so it may
/// point past the end of `data`) up to `max_size` bytes. `start` is clamped
/// to `data.len()` before slicing so that an empty (or short) buffer with
/// an out-of-range `start` never panics.
fn slice_buffer(data: &[u8], start: usize, max_size: usize) -> Vec<u8> {
    let start = start.min(data.len());
    let end = data.len().min(start.saturating_add(max_size));
    data[start..end].to_vec()
}

// ---------- guest memory helpers ----------

fn guest_memory(caller: &mut Caller<'_, HostState>) -> Option<wasmtime::Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
}

fn read_guest(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<Vec<u8>, i32> {
    if ptr < 0 || len < 0 {
        return Err(STATUS_BAD_ARGUMENT);
    }
    let mem = guest_memory(caller).ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let (ptr, len) = (ptr as usize, len as usize);
    // Bounds-check against the guest's ACTUAL memory size BEFORE allocating.
    // `len` is guest-controlled (up to ~2GiB); a blind `vec![0u8; len]` would
    // let a guest drive the host toward OOM before the subsequent `mem.read`
    // bounds check ever fires. Same hazard `decode_map` guards against, applied
    // one level lower so every guest-input ABI (dispatch, set_shared_data,
    // set_header_map_pairs, send_local_response, …) is covered.
    let mem_size = mem.data_size(&*caller);
    if ptr.checked_add(len).map_or(true, |end| end > mem_size) {
        return Err(STATUS_INVALID_MEMORY_ACCESS);
    }
    let mut buf = vec![0u8; len];
    mem.read(&mut *caller, ptr, &mut buf)
        .map_err(|_| STATUS_INVALID_MEMORY_ACCESS)?;
    Ok(buf)
}

fn read_guest_string(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<String, i32> {
    Ok(String::from_utf8_lossy(&read_guest(caller, ptr, len)?).into_owned())
}

/// Allocate `data.len()` bytes in the guest via its `proxy_on_memory_allocate`
/// (fallback `malloc`) export, copy `data` in, and write the resulting
/// (ptr, size) u32 LE pair at `ret_ptr` / `ret_size_ptr`.
fn write_to_guest(caller: &mut Caller<'_, HostState>, data: &[u8], ret_ptr: i32, ret_size_ptr: i32) -> i32 {
    let Some(Extern::Func(alloc)) = caller
        .get_export("proxy_on_memory_allocate")
        .or_else(|| caller.get_export("malloc"))
    else {
        return STATUS_INTERNAL_FAILURE;
    };
    let Ok(alloc) = alloc.typed::<i32, i32>(&mut *caller) else {
        return STATUS_INTERNAL_FAILURE;
    };
    let Ok(guest_ptr) = alloc.call(&mut *caller, data.len() as i32) else {
        return STATUS_INTERNAL_FAILURE;
    };
    let Some(mem) = guest_memory(caller) else {
        return STATUS_INVALID_MEMORY_ACCESS;
    };
    if mem.write(&mut *caller, guest_ptr as usize, data).is_err() {
        return STATUS_INVALID_MEMORY_ACCESS;
    }
    if mem
        .write(&mut *caller, ret_ptr as usize, &(guest_ptr as u32).to_le_bytes())
        .is_err()
        || mem
            .write(&mut *caller, ret_size_ptr as usize, &(data.len() as u32).to_le_bytes())
            .is_err()
    {
        return STATUS_INVALID_MEMORY_ACCESS;
    }
    STATUS_OK
}

/// Host handler for the guest callout dispatch (`proxy_http_call` /
/// `proxy_dispatch_http_call`). Queues the call in `HostState`; an async
/// host path (the plugin's request path while the guest is paused, or the
/// module tick task after `on_tick`) executes it and delivers the result via
/// `proxy_on_http_call_response`. Sync stages (response headers/body) never
/// arm dispatch — and the tick task arms it only when the module's plugins
/// agree on one non-empty allowlist — so the guest sees UNIMPLEMENTED
/// elsewhere instead of a silent no-op.
///
/// A free `fn` (not a closure) so the same handler can be registered under
/// both import names.
#[allow(clippy::too_many_arguments)]
fn host_http_call(
    mut caller: Caller<'_, HostState>,
    upstream_ptr: i32,
    upstream_size: i32,
    headers_ptr: i32,
    headers_size: i32,
    body_ptr: i32,
    body_size: i32,
    _trailers_ptr: i32,
    _trailers_size: i32,
    timeout_ms: i32,
    return_token_ptr: i32,
) -> i32 {
    if !caller.data().dispatch_enabled {
        return STATUS_UNIMPLEMENTED;
    }
    let upstream = match read_guest_string(&mut caller, upstream_ptr, upstream_size) {
        Ok(s) => s,
        Err(s) => return s,
    };
    let header_bytes = match read_guest(&mut caller, headers_ptr, headers_size) {
        Ok(b) => b,
        Err(s) => return s,
    };
    let Some(all_headers) = decode_map(&header_bytes) else {
        return STATUS_BAD_ARGUMENT;
    };
    let body = if body_size > 0 {
        match read_guest(&mut caller, body_ptr, body_size) {
            Ok(b) => b,
            Err(s) => return s,
        }
    } else {
        Vec::new()
    };

    // The upstream parameter addresses the target directly as an origin URL
    // (Edgion has no Envoy-style cluster registry for guests to name).
    let origin = match edgion_resources::plugin_configs::normalize_callout_origin(&upstream) {
        Ok(o) => o,
        // Data-plane hot path (Iron rule 3), guest-supplied origin (Iron rule 1):
        // reject via status code only, no tracing.
        Err(_) => return STATUS_BAD_ARGUMENT,
    };

    let state = caller.data_mut();
    if !state.callout_allowlist.iter().any(|a| a == &origin) {
        // Bounded metric carries the signal; the guest-supplied origin must not
        // reach tracing on the request path.
        crate::observe::metrics::registry::record_wasm_callout(&state.metrics_label, "denied");
        return STATUS_INTERNAL_FAILURE;
    }
    if state.callouts_dispatched >= MAX_CALLOUTS_PER_REQUEST {
        crate::observe::metrics::registry::record_wasm_callout(&state.metrics_label, "over_limit");
        return STATUS_INTERNAL_FAILURE;
    }

    let mut method = "GET".to_string();
    let mut path = "/".to_string();
    let mut authority = None;
    let mut headers = Vec::with_capacity(all_headers.len());
    for (name, value) in all_headers {
        match name.as_str() {
            ":method" => method = value,
            ":path" => path = value,
            ":authority" => authority = Some(value),
            n if n.starts_with(':') => {}
            _ => headers.push((name, value)),
        }
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }

    let token = state.next_callout_token;
    let pending = PendingCallout {
        token,
        origin,
        path,
        method,
        authority,
        headers,
        body,
        timeout_ms: timeout_ms.max(0) as u32,
    };

    // Deliver the token to the guest FIRST; only commit the dispatch (increment
    // counter + queue) if the write succeeds. A bad return_token_ptr must not
    // leave a phantom callout queued (and a consumed cap slot) that the guest
    // never received a handle for.
    let Some(mem) = guest_memory(&mut caller) else {
        return STATUS_INVALID_MEMORY_ACCESS;
    };
    if mem
        .write(&mut caller, return_token_ptr as usize, &token.to_le_bytes())
        .is_err()
    {
        return STATUS_INVALID_MEMORY_ACCESS;
    }
    let state = caller.data_mut();
    state.callouts_dispatched += 1;
    state.next_callout_token = token.wrapping_add(1);
    state.pending_callouts.push(pending);
    STATUS_OK
}

/// Register the implemented proxy-wasm host functions. Call
/// `linker.define_unknown_imports_as_traps(&module)` AFTER this so any
/// import outside the v1 subset traps only if the guest actually calls it.
pub fn add_to_linker(linker: &mut Linker<HostState>) -> anyhow::Result<()> {
    linker.func_wrap(
        "env",
        "proxy_log",
        |mut caller: Caller<'_, HostState>, level: i32, ptr: i32, len: i32| -> i32 {
            match read_guest_string(&mut caller, ptr, len) {
                Ok(msg) => {
                    // Data-plane hot path (Iron rule 3) and a guest-controlled
                    // message (Iron rule 1): never tracing the raw text. The
                    // line is stored for bounded in-memory diagnostics only and
                    // reported as a count via PluginLog by `finish_phase`.
                    let state = caller.data_mut();
                    if let Err(s) = state.charge_abi(msg.len()) {
                        return s;
                    }
                    state.log_lines.push((level, msg));
                    STATUS_OK
                }
                Err(s) => s,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_get_current_time_nanoseconds",
        |mut caller: Caller<'_, HostState>, ret_ptr: i32| -> i32 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let Some(mem) = guest_memory(&mut caller) else {
                return STATUS_INVALID_MEMORY_ACCESS;
            };
            match mem.write(&mut caller, ret_ptr as usize, &nanos.to_le_bytes()) {
                Ok(_) => STATUS_OK,
                Err(_) => STATUS_INVALID_MEMORY_ACCESS,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_get_header_map_pairs",
        |mut caller: Caller<'_, HostState>, map: i32, ret_ptr: i32, ret_size: i32| -> i32 {
            // Encode into an owned buffer inside a scope so the immutable
            // borrow of the header map is released before write_to_guest takes
            // caller mutably.
            let encoded = {
                let Some(pairs) = caller.data().map_ref(map) else {
                    return STATUS_BAD_ARGUMENT;
                };
                encode_map(pairs)
            };
            write_to_guest(&mut caller, &encoded, ret_ptr, ret_size)
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_get_header_map_value",
        |mut caller: Caller<'_, HostState>, map: i32, kp: i32, kl: i32, ret_ptr: i32, ret_size: i32| -> i32 {
            let key = match read_guest_string(&mut caller, kp, kl) {
                Ok(k) => k,
                Err(s) => return s,
            };
            let value = {
                let Some(pairs) = caller.data().map_ref(map) else {
                    return STATUS_BAD_ARGUMENT;
                };
                // Clone only the single matched value (not the whole map).
                pairs
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&key))
                    .map(|(_, v)| v.clone())
            };
            match value {
                Some(v) => write_to_guest(&mut caller, v.as_bytes(), ret_ptr, ret_size),
                None => STATUS_NOT_FOUND,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_replace_header_map_value",
        |mut caller: Caller<'_, HostState>, map: i32, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
            let key = match read_guest_string(&mut caller, kp, kl) {
                Ok(k) => k,
                Err(s) => return s,
            };
            let value = match read_guest_string(&mut caller, vp, vl) {
                Ok(v) => v,
                Err(s) => return s,
            };
            let state = caller.data_mut();
            // Charge the retained `pending_ops` op (append-only) before any
            // mutation, so a rejected op leaves no partial header change.
            if let Err(s) = state.charge_abi(key.len() + value.len()) {
                return s;
            }
            let Some(pairs) = state.map_mut(map) else {
                return STATUS_BAD_ARGUMENT;
            };
            pairs.retain(|(n, _)| !n.eq_ignore_ascii_case(&key));
            pairs.push((key.clone(), value.clone()));
            state.pending_ops.push(HeaderOp::Set { map, name: key, value });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_add_header_map_value",
        |mut caller: Caller<'_, HostState>, map: i32, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
            let key = match read_guest_string(&mut caller, kp, kl) {
                Ok(k) => k,
                Err(s) => return s,
            };
            let value = match read_guest_string(&mut caller, vp, vl) {
                Ok(v) => v,
                Err(s) => return s,
            };
            let state = caller.data_mut();
            // Append-only in both the snapshot and `pending_ops` (the strongest
            // amplifier): charge before mutating so a rejected op is atomic.
            if let Err(s) = state.charge_abi(key.len() + value.len()) {
                return s;
            }
            let Some(pairs) = state.map_mut(map) else {
                return STATUS_BAD_ARGUMENT;
            };
            pairs.push((key.clone(), value.clone()));
            state.pending_ops.push(HeaderOp::Add { map, name: key, value });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_remove_header_map_value",
        |mut caller: Caller<'_, HostState>, map: i32, kp: i32, kl: i32| -> i32 {
            let key = match read_guest_string(&mut caller, kp, kl) {
                Ok(k) => k,
                Err(s) => return s,
            };
            let state = caller.data_mut();
            // Charge the retained `pending_ops` Remove op before mutating.
            if let Err(s) = state.charge_abi(key.len()) {
                return s;
            }
            let Some(pairs) = state.map_mut(map) else {
                return STATUS_BAD_ARGUMENT;
            };
            pairs.retain(|(n, _)| !n.eq_ignore_ascii_case(&key));
            state.pending_ops.push(HeaderOp::Remove { map, name: key });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_set_header_map_pairs",
        |mut caller: Caller<'_, HostState>, map: i32, ptr: i32, len: i32| -> i32 {
            let bytes = match read_guest(&mut caller, ptr, len) {
                Ok(b) => b,
                Err(s) => return s,
            };
            let Some(new_pairs) = decode_map(&bytes) else {
                return STATUS_BAD_ARGUMENT;
            };
            let state = caller.data_mut();
            // Each call appends a whole owned `ReplaceAll` op to `pending_ops`
            // (append-only) — a strong amplifier. Charge the new payload before
            // mutating so a rejected op leaves the snapshot untouched.
            let payload: usize = new_pairs.iter().map(|(k, v)| k.len() + v.len()).sum();
            if let Err(s) = state.charge_abi(payload) {
                return s;
            }
            let Some(pairs) = state.map_mut(map) else {
                return STATUS_BAD_ARGUMENT;
            };
            // Capture the names present before the replacement (case-insensitive
            // dedup) so apply_ops removes every header the guest dropped. One
            // remove per unique name suffices — the session remove clears all
            // values for that name. The snapshot only holds str-representable
            // headers (the session filters non-UTF-8 values out before the
            // guest ever sees them), so a header the guest cannot observe is
            // also not captured here — realistic credential/identity headers
            // are ASCII and are covered.
            let mut remove: Vec<String> = Vec::new();
            for (name, _) in pairs.iter() {
                if !remove.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                    remove.push(name.clone());
                }
            }
            *pairs = new_pairs.clone();
            state.pending_ops.push(HeaderOp::ReplaceAll {
                map,
                remove,
                pairs: new_pairs,
            });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_get_buffer_bytes",
        |mut caller: Caller<'_, HostState>,
         buf_type: i32,
         start: i32,
         max_size: i32,
         ret_ptr: i32,
         ret_size: i32|
         -> i32 {
            let data = match buf_type {
                BUF_VM_CONFIGURATION => caller.data().vm_config.clone(),
                BUF_PLUGIN_CONFIGURATION => caller.data().plugin_config.clone(),
                BUF_HTTP_CALL_RESPONSE_BODY => caller
                    .data()
                    .callout_response
                    .as_ref()
                    .map(|r| r.body.clone())
                    .unwrap_or_default(),
                // Read-only chunk window (body observation): the CURRENT
                // chunk only, never an accumulated body.
                BUF_HTTP_RESPONSE_BODY => caller.data().body_chunk.clone(),
                _ => return STATUS_NOT_FOUND,
            };
            // proxy-wasm treats start/max_size as unsigned; the Rust SDK's
            // get_plugin_configuration passes usize::MAX (wasm32: u32::MAX,
            // i.e. -1 as i32) for "the whole buffer". Interpret as u32 —
            // a signed max(0) clamp would silently read 0 bytes there.
            let start = start as u32 as usize;
            if start >= data.len() && !data.is_empty() {
                return STATUS_BAD_ARGUMENT;
            }
            let slice = slice_buffer(&data, start, max_size as u32 as usize);
            write_to_guest(&mut caller, &slice, ret_ptr, ret_size)
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_send_local_response",
        |mut caller: Caller<'_, HostState>,
         status: i32,
         _detail_ptr: i32,
         _detail_size: i32,
         body_ptr: i32,
         body_size: i32,
         headers_ptr: i32,
         headers_size: i32,
         _grpc_status: i32|
         -> i32 {
            let body = if body_size > 0 {
                match read_guest(&mut caller, body_ptr, body_size) {
                    Ok(b) => b,
                    Err(s) => return s,
                }
            } else {
                Vec::new()
            };
            let headers = if headers_size > 0 {
                let bytes = match read_guest(&mut caller, headers_ptr, headers_size) {
                    Ok(b) => b,
                    Err(s) => return s,
                };
                decode_map(&bytes).unwrap_or_default()
            } else {
                Vec::new()
            };
            let status = if (100..=599).contains(&status) {
                status as u16
            } else {
                500
            };
            // Replacement state (a single Option), not an accumulator: repeated
            // calls overwrite rather than grow, so its peak is one body, bounded
            // by the `ResourceLimiter`-capped guest linear memory. Not charged
            // against the append-only ABI retention budget.
            caller.data_mut().local_response = Some(LocalResponse { status, body, headers });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_get_property",
        |mut caller: Caller<'_, HostState>, pp: i32, pl: i32, ret_ptr: i32, ret_size: i32| -> i32 {
            let raw = match read_guest(&mut caller, pp, pl) {
                Ok(b) => b,
                Err(s) => return s,
            };
            // proxy-wasm property paths are NUL-separated segments.
            let key = raw
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect::<Vec<_>>()
                .join(".");
            match caller.data().properties.get(&key).cloned() {
                Some(v) => write_to_guest(&mut caller, v.as_bytes(), ret_ptr, ret_size),
                None => STATUS_NOT_FOUND,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_set_property",
        |mut caller: Caller<'_, HostState>, pp: i32, pl: i32, vp: i32, vl: i32| -> i32 {
            let raw = match read_guest(&mut caller, pp, pl) {
                Ok(b) => b,
                Err(s) => return s,
            };
            let key = raw
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect::<Vec<_>>()
                .join(".");
            let value = match read_guest_string(&mut caller, vp, vl) {
                Ok(v) => v,
                Err(s) => return s,
            };
            let state = caller.data_mut();
            // Distinct guest-generated keys grow `properties` unboundedly.
            // Charge before inserting so a rejected set does not grow the map.
            if let Err(s) = state.charge_abi(key.len() + value.len()) {
                return s;
            }
            state.properties.insert(key, value);
            STATUS_OK
        },
    )?;

    // Legacy 0.1 configuration accessor (some SDKs call it from on_configure).
    linker.func_wrap(
        "env",
        "proxy_get_configuration",
        |mut caller: Caller<'_, HostState>, ret_ptr: i32, ret_size: i32| -> i32 {
            let data = caller.data().plugin_config.clone();
            write_to_guest(&mut caller, &data, ret_ptr, ret_size)
        },
    )?;

    // HTTP callout dispatch — see `host_http_call`. The proxy-wasm ABI names
    // this import `proxy_http_call` (what the official
    // Rust/C++ SDKs actually call). Older/hand-written guests sometimes spell it
    // `proxy_dispatch_http_call`; register the same handler under both so an SDK
    // guest is not silently trapped by `define_unknown_imports_as_traps`.
    linker.func_wrap("env", "proxy_http_call", host_http_call)?;
    linker.func_wrap("env", "proxy_dispatch_http_call", host_http_call)?;

    // Shared data (per-module KV with CAS). Process-local: shared across
    // all pooled VMs of one ModuleEntry, freed with the cache entry.
    linker.func_wrap(
        "env",
        "proxy_get_shared_data",
        |mut caller: Caller<'_, HostState>,
         key_ptr: i32,
         key_size: i32,
         ret_value_ptr: i32,
         ret_value_size: i32,
         ret_cas_ptr: i32|
         -> i32 {
            let key = match read_guest_string(&mut caller, key_ptr, key_size) {
                Ok(k) => k,
                Err(s) => return s,
            };
            let Some((value, cas)) = caller.data().shared_data.get(&key) else {
                return STATUS_NOT_FOUND;
            };
            let status = write_to_guest(&mut caller, &value, ret_value_ptr, ret_value_size);
            if status != STATUS_OK {
                return status;
            }
            let Some(mem) = guest_memory(&mut caller) else {
                return STATUS_INVALID_MEMORY_ACCESS;
            };
            match mem.write(&mut caller, ret_cas_ptr as usize, &cas.to_le_bytes()) {
                Ok(_) => STATUS_OK,
                Err(_) => STATUS_INVALID_MEMORY_ACCESS,
            }
        },
    )?;

    linker.func_wrap(
        "env",
        "proxy_set_shared_data",
        |mut caller: Caller<'_, HostState>,
         key_ptr: i32,
         key_size: i32,
         value_ptr: i32,
         value_size: i32,
         cas: i32|
         -> i32 {
            let key = match read_guest_string(&mut caller, key_ptr, key_size) {
                Ok(k) => k,
                Err(s) => return s,
            };
            // Null value = delete (proxy-wasm convention).
            let value = if value_ptr == 0 && value_size == 0 {
                None
            } else {
                match read_guest(&mut caller, value_ptr, value_size) {
                    Ok(v) => Some(v),
                    Err(s) => return s,
                }
            };
            use super::shared_data::SharedDataError;
            match caller.data().shared_data.set(&key, value, cas as u32) {
                Ok(()) => STATUS_OK,
                Err(SharedDataError::CasMismatch) => STATUS_CAS_MISMATCH,
                Err(SharedDataError::TooLarge) => STATUS_BAD_ARGUMENT,
            }
        },
    )?;

    // Accepted no-ops for the v1 subset.
    linker.func_wrap(
        "env",
        "proxy_set_effective_context",
        |_: Caller<'_, HostState>, _ctx: i32| -> i32 { STATUS_OK },
    )?;
    linker.func_wrap(
        "env",
        "proxy_set_tick_period_milliseconds",
        |mut caller: Caller<'_, HostState>, ms: i32| -> i32 {
            caller.data_mut().tick_period_ms = ms.max(0) as u64;
            STATUS_OK
        },
    )?;
    linker.func_wrap("env", "proxy_done", |_: Caller<'_, HostState>| -> i32 { STATUS_OK })?;
    // Pause/resume: the guest resumes a paused request either through the
    // 0.1-style proxy_continue_request or the 0.2-style
    // proxy_continue_stream(stream_type=0 /* Request */).
    linker.func_wrap(
        "env",
        "proxy_continue_stream",
        |mut caller: Caller<'_, HostState>, stream: i32| -> i32 {
            if stream == 0 {
                caller.data_mut().continue_requested = true;
            }
            STATUS_OK
        },
    )?;
    linker.func_wrap(
        "env",
        "proxy_continue_request",
        |mut caller: Caller<'_, HostState>| -> i32 {
            caller.data_mut().continue_requested = true;
            STATUS_OK
        },
    )?;
    linker.func_wrap("env", "proxy_continue_response", |_: Caller<'_, HostState>| -> i32 {
        STATUS_OK
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Engine, Instance, Module, Store};

    /// Minimal guest: bump allocator + on request headers sets the header
    /// `x-wat-added: yes`, and on response headers sends a 403 local
    /// response with body "denied".
    const GUEST: &str = r#"
    (module
      (import "env" "proxy_replace_header_map_value"
        (func $set_hdr (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $slr (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 4096))
      ;; offsets: 0: "x-wat-added" (11), 16: "yes" (3), 32: "denied" (6)
      (data (i32.const 0) "x-wat-added")
      (data (i32.const 16) "yes")
      (data (i32.const 32) "denied")
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
        (drop (call $set_hdr (i32.const 0) (i32.const 0) (i32.const 11) (i32.const 16) (i32.const 3)))
        (i32.const 0))
      (func (export "proxy_on_response_headers") (param i32 i32 i32) (result i32)
        (drop (call $slr (i32.const 403) (i32.const 0) (i32.const 0)
                         (i32.const 32) (i32.const 6) (i32.const 0) (i32.const 0) (i32.const 0)))
        (i32.const 0))
      (func (export "proxy_on_done") (param i32) (result i32) (i32.const 1))
      (func (export "proxy_on_delete") (param i32))
    )
    "#;

    fn instantiate() -> (Store<HostState>, Instance) {
        let engine = Engine::default();
        let module = Module::new(&engine, GUEST).expect("wat compiles");
        let mut linker: Linker<HostState> = Linker::new(&engine);
        add_to_linker(&mut linker).unwrap();
        let mut store = Store::new(&engine, HostState::default());
        let instance = linker.instantiate(&mut store, &module).unwrap();
        (store, instance)
    }

    #[test]
    fn map_codec_roundtrip() {
        let pairs = vec![
            (":path".to_string(), "/a b".to_string()),
            ("x-empty".to_string(), String::new()),
        ];
        assert_eq!(decode_map(&encode_map(&pairs)).unwrap(), pairs);
        assert_eq!(decode_map(&encode_map(&[])).unwrap(), Vec::new());
        assert!(decode_map(&[1, 2]).is_none());
    }

    /// A malformed guest payload claiming an absurd element count must be
    /// rejected by length validation, not by pre-allocating gigabytes of
    /// memory (which would trip Rust's alloc-error handler and abort the
    /// whole host process). Reachable via `proxy_set_header_map_pairs`.
    #[test]
    fn decode_map_huge_count_returns_none_without_abort() {
        let mut b = vec![0xFF, 0xFF, 0xFF, 0xFF];
        b.extend_from_slice(&[0u8; 8]);
        assert!(decode_map(&b).is_none());
    }

    /// `start` is guest-controlled and may point past the end of `data`
    /// (e.g. an empty plugin/VM configuration buffer with any nonzero
    /// `start`). Slicing must never panic.
    #[test]
    fn slice_buffer_empty_data_nonzero_start_no_panic() {
        assert_eq!(slice_buffer(&[], 3, 10), Vec::<u8>::new());
    }

    #[test]
    fn slice_buffer_normal_cases() {
        let data = b"hello world";
        assert_eq!(slice_buffer(data, 0, 5), b"hello".to_vec());
        assert_eq!(slice_buffer(data, 6, 100), b"world".to_vec());
        assert_eq!(slice_buffer(data, 100, 5), Vec::<u8>::new());
    }

    #[test]
    fn guest_set_header_queues_op_and_mutates_snapshot() {
        let (mut store, instance) = instantiate();
        store.data_mut().begin_phase(
            WasmPhase::RequestHeaders,
            vec![("host".to_string(), "example.com".to_string())],
            vec![],
            HashMap::new(),
        );
        let f = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "proxy_on_request_headers")
            .unwrap();
        let action = f.call(&mut store, (2, 1, 1)).unwrap();
        assert_eq!(action, 0); // Continue
        let state = store.data();
        assert!(state
            .request_headers
            .iter()
            .any(|(k, v)| k == "x-wat-added" && v == "yes"));
        assert!(matches!(
            state.pending_ops.as_slice(),
            [HeaderOp::Set { map: MAP_HTTP_REQUEST_HEADERS, name, value }]
                if name == "x-wat-added" && value == "yes"
        ));
    }

    #[test]
    fn guest_send_local_response_recorded() {
        let (mut store, instance) = instantiate();
        store
            .data_mut()
            .begin_phase(WasmPhase::ResponseHeaders, vec![], vec![], HashMap::new());
        let f = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "proxy_on_response_headers")
            .unwrap();
        f.call(&mut store, (2, 0, 1)).unwrap();
        let lr = store.data().local_response.as_ref().expect("local response set");
        assert_eq!(lr.status, 403);
        assert_eq!(lr.body, b"denied");
    }

    /// A single payload larger than the byte budget is rejected, and the
    /// rejection charges nothing (no partial mutation of the counters).
    #[test]
    fn charge_abi_rejects_over_byte_budget_atomically() {
        let mut s = HostState::default();
        assert_eq!(s.charge_abi(MAX_RETAINED_ABI_BYTES), Err(STATUS_INTERNAL_FAILURE));
        assert_eq!(s.retained_bytes, 0);
        assert_eq!(s.retained_ops, 0);
        // A payload that fits is charged with the fixed per-op overhead.
        assert_eq!(s.charge_abi(10), Ok(()));
        assert_eq!(s.retained_bytes, 10 + ABI_OP_OVERHEAD_BYTES);
        assert_eq!(s.retained_ops, 1);
    }

    /// A flood of zero-byte operations is bounded by the op-count budget even
    /// though the byte budget is nowhere near exhausted.
    #[test]
    fn charge_abi_rejects_over_op_budget() {
        let mut s = HostState::default();
        for _ in 0..MAX_RETAINED_ABI_OPS {
            assert_eq!(s.charge_abi(0), Ok(()));
        }
        assert_eq!(s.charge_abi(0), Err(STATUS_INTERNAL_FAILURE));
        assert_eq!(s.retained_ops, MAX_RETAINED_ABI_OPS);
        assert!(s.retained_bytes < MAX_RETAINED_ABI_BYTES);
    }

    fn instantiate_guest(wat: &str) -> (Store<HostState>, Instance) {
        let engine = Engine::default();
        let module = Module::new(&engine, wat).expect("wat compiles");
        let mut linker: Linker<HostState> = Linker::new(&engine);
        add_to_linker(&mut linker).unwrap();
        let mut store = Store::new(&engine, HostState::default());
        let instance = linker.instantiate(&mut store, &module).unwrap();
        (store, instance)
    }

    /// Adversarial guest: on request headers it loops calling `proxy_log` with a
    /// fixed 1 MiB buffer until a call returns non-zero, then stops (safety cap
    /// 5000 iterations). The engine here has NO epoch interruption, so the ONLY
    /// bound is the per-phase byte budget — this demonstrates the memory budget,
    /// not an execution-time bound, is what stops the accumulation.
    const LOG_FLOOD_GUEST: &str = r#"
    (module
      (import "env" "proxy_log" (func $log (param i32 i32 i32) (result i32)))
      (memory (export "memory") 16) ;; 16 pages = 1 MiB
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (local $i i32) (local $rc i32)
        (block $done
          (loop $l
            (local.set $rc (call $log (i32.const 1) (i32.const 0) (i32.const 1048576)))
            (br_if $done (local.get $rc))
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br_if $l (i32.lt_u (local.get $i) (i32.const 5000)))))
        (i32.const 0))
    )
    "#;

    #[test]
    fn guest_log_flood_stopped_by_byte_budget() {
        let (mut store, instance) = instantiate_guest(LOG_FLOOD_GUEST);
        store
            .data_mut()
            .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
        let f = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "proxy_on_request_headers")
            .unwrap();
        f.call(&mut store, (2, 0, 0)).unwrap();
        let state = store.data();
        // ~32 successful 1 MiB lines fit under the 32 MiB budget; the flood
        // stops well before the 5000-iteration safety cap.
        assert!(state.log_lines.len() >= 30 && state.log_lines.len() <= 34);
        assert!(state.retained_bytes <= MAX_RETAINED_ABI_BYTES);
    }

    /// Adversarial guest: loops `proxy_add_header_map_value` with a fixed ~1 MiB
    /// value until rejected. Asserts the snapshot and `pending_ops` stay in
    /// lockstep — a rejected op leaves NO partial header mutation.
    const ADD_HEADER_FLOOD_GUEST: &str = r#"
    (module
      (import "env" "proxy_add_header_map_value"
        (func $add (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 16)
      (data (i32.const 0) "k")
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_request_headers") (param i32 i32 i32) (result i32)
        (local $i i32) (local $rc i32)
        (block $done
          (loop $l
            ;; add(map=0 request, key_ptr=0 key_len=1, val_ptr=8 val_len=1000000)
            (local.set $rc (call $add (i32.const 0) (i32.const 0) (i32.const 1)
                                      (i32.const 8) (i32.const 1000000)))
            (br_if $done (local.get $rc))
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br_if $l (i32.lt_u (local.get $i) (i32.const 5000)))))
        (i32.const 0))
    )
    "#;

    #[test]
    fn guest_add_header_flood_no_partial_mutation() {
        let (mut store, instance) = instantiate_guest(ADD_HEADER_FLOOD_GUEST);
        store
            .data_mut()
            .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
        let f = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "proxy_on_request_headers")
            .unwrap();
        f.call(&mut store, (2, 0, 0)).unwrap();
        let state = store.data();
        // Every successful add pushes exactly one snapshot entry AND one
        // pending_op; a rejected add pushes neither. Lockstep counts prove the
        // rejected op left no partial mutation.
        assert!(!state.pending_ops.is_empty());
        assert_eq!(state.request_headers.len(), state.pending_ops.len());
        assert!(state.request_headers.len() < 5000); // stopped by budget, not the cap
    }
}
