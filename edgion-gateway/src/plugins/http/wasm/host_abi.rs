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

use wasmtime::{Caller, Extern, FuncType, Linker, ValType};

use edgion_wasm::{host, HostBehavior, WasmValueType};

pub const MAP_HTTP_REQUEST_HEADERS: i32 = 0;
pub const MAP_HTTP_RESPONSE_HEADERS: i32 = 2;
pub const MAP_HTTP_CALL_RESPONSE_HEADERS: i32 = 6;

/// proxy-wasm `BufferType::HttpRequestBody`. Numerically 0 like
/// `MAP_HTTP_REQUEST_HEADERS`, but a distinct enum (buffer-type vs map-id),
/// so there is no collision.
pub const BUF_HTTP_REQUEST_BODY: i32 = 0;
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
/// to `pending_ops` / `pending_callouts` / `log_lines` / `properties` until
/// `begin_phase` clears them after the hook. Epoch interruption bounds
/// execution TIME, not allocation. This budget bounds the host-heap peak a
/// single hook can drive.
/// 32 MiB is far above any legitimate header/log/property working set; only a
/// pathological accumulation loop trips it.
pub const MAX_RETAINED_ABI_BYTES: usize = 32 * 1024 * 1024;

/// Aggregate per-phase budget for host-retained ABI operation count. Bounds a
/// flood of tiny (near-zero-byte) operations whose per-item structural
/// overhead the byte budget alone under-charges.
pub const MAX_RETAINED_ABI_OPS: usize = 200_000;

/// Aggregate request-body mutation work allowed in one request-body hook,
/// measured in passes over the request's resolved physical Body cap.
///
/// Work is charged as replacement bytes plus the tail bytes that `Vec::splice`
/// must move when an edit changes length. Two passes permit two equal-size
/// full rewrites while preventing a small configured Body cap from inheriting
/// a fixed, much larger mutation allowance.
const MAX_REQUEST_BODY_MUTATION_WORK_PASSES: usize = 2;

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
    /// Request-body buffer-mode hook: the full request body has been buffered
    /// by the body gate and is driven to the guest once with end_of_stream.
    RequestBody,
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

    // ---- Body state ----
    /// Current response-body chunk exposed via
    /// `get_buffer_bytes(HttpResponseBody)`. Never buffers across chunks.
    pub body_chunk: Vec<u8>,

    /// Full request body staged for buffer mode, exposed via
    /// `get_buffer_bytes(HttpRequestBody)`. Populated once from the body gate's
    /// buffered bytes; never a per-chunk window. Request-body mutations are
    /// applied to this staging buffer and published atomically after the guest
    /// hook returns.
    pub request_body: Vec<u8>,
    /// Gateway physical Body cap resolved for the current request-body phase.
    /// Zero outside that phase.
    pub request_body_limit: usize,
    /// Whether `proxy_set_buffer_bytes(HttpRequestBody, ...)` changed the
    /// staging buffer during the current hook.
    pub request_body_modified: bool,
    /// Cumulative copy/move work charged for request-body mutations in the
    /// current hook. This is separate from retained heap because successful
    /// replacements retain only the final bounded staging buffer.
    request_body_mutation_work: usize,

    /// Guest-requested tick period in ms (0 = no tick). Module-lifetime
    /// state (typically set during `on_configure`), NOT reset per phase;
    /// the module tick task reads it after instance init.
    pub tick_period_ms: u64,

    /// Module-wide shared KV (proxy-wasm shared data). Shared by every
    /// pooled VM of the same `ModuleEntry`; process-local by design.
    pub shared_data: Arc<super::shared_data::SharedData>,

    // ---- Per-phase ABI retention budget (see `charge_abi`) ----
    /// Host-retained ABI payload bytes charged this phase across the
    /// append-only accumulators (`pending_ops`, `pending_callouts`,
    /// `log_lines`, `properties`).
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
        self.request_body.clear();
        self.request_body_limit = 0;
        self.request_body_modified = false;
        self.request_body_mutation_work = 0;
        self.retained_bytes = 0;
        self.retained_ops = 0;
    }

    /// Charge `payload` bytes plus fixed per-op overhead against the per-phase
    /// ABI retention budget. Returns `Err(STATUS_INTERNAL_FAILURE)` when either
    /// the byte or op budget would be exceeded — WITHOUT mutating any counter —
    /// so the caller rejects before touching the snapshot (no partial
    /// mutation). Commits the charge and returns `Ok(())` otherwise.
    fn charge_abi(&mut self, payload: usize) -> Result<(), i32> {
        let checked = self.checked_abi_charge(payload)?;
        self.commit_abi_charge(checked);
        Ok(())
    }

    fn checked_abi_charge(&self, payload: usize) -> Result<(usize, usize), i32> {
        self.checked_abi_charge_with_limits(payload, MAX_RETAINED_ABI_BYTES, MAX_RETAINED_ABI_OPS)
    }

    fn checked_abi_charge_with_limits(
        &self,
        payload: usize,
        max_bytes: usize,
        max_ops: usize,
    ) -> Result<(usize, usize), i32> {
        let cost = payload.saturating_add(ABI_OP_OVERHEAD_BYTES);
        let new_bytes = self.retained_bytes.saturating_add(cost);
        let new_ops = self.retained_ops.saturating_add(1);
        if new_bytes > max_bytes || new_ops > max_ops {
            return Err(STATUS_INTERNAL_FAILURE);
        }
        Ok((new_bytes, new_ops))
    }

    fn commit_abi_charge(&mut self, checked: (usize, usize)) {
        self.retained_bytes = checked.0;
        self.retained_ops = checked.1;
    }

    /// Validate a request-body mutation charge without changing counters.
    ///
    /// The ABI bridge uses this before copying guest memory so a guest cannot
    /// keep forcing large transient reads after exhausting the work budget.
    fn check_request_body_mutation_charge(&self, work: usize) -> Result<(), i32> {
        self.checked_abi_charge(0)?;
        checked_request_body_mutation_total(self.request_body_mutation_work, work, self.request_body_limit).map(drop)
    }

    /// Commit a previously validated request-body mutation charge.
    fn charge_request_body_mutation(&mut self, work: usize) -> Result<(), i32> {
        let new_work =
            checked_request_body_mutation_total(self.request_body_mutation_work, work, self.request_body_limit)?;
        self.charge_abi(0)?;
        self.request_body_mutation_work = new_work;
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

#[derive(Clone, Copy)]
struct SerializedMapProfile {
    pair_count: usize,
    lossy_string_bytes: usize,
    path_normalization_bytes: usize,
}

fn lossy_utf8_output_len(mut bytes: &[u8]) -> Result<usize, i32> {
    let mut output_len = 0usize;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(_) => {
                return output_len.checked_add(bytes.len()).ok_or(STATUS_INTERNAL_FAILURE);
            }
            Err(error) => {
                let valid = error.valid_up_to();
                output_len = output_len
                    .checked_add(valid)
                    .and_then(|len| len.checked_add('\u{FFFD}'.len_utf8()))
                    .ok_or(STATUS_INTERNAL_FAILURE)?;
                let invalid = error.error_len().unwrap_or(bytes.len().saturating_sub(valid));
                bytes = bytes
                    .get(valid.saturating_add(invalid)..)
                    .ok_or(STATUS_INTERNAL_FAILURE)?;
            }
        }
    }
}

fn string_from_utf8_lossy_with_capacity(bytes: &[u8], capacity: usize) -> String {
    let mut output = String::with_capacity(capacity);
    let mut remaining = bytes;
    loop {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                output.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if let Ok(valid_prefix) = std::str::from_utf8(&remaining[..valid]) {
                    output.push_str(valid_prefix);
                }
                output.push('\u{FFFD}');
                let invalid = error.error_len().unwrap_or(remaining.len() - valid);
                remaining = &remaining[valid + invalid..];
            }
        }
    }
    debug_assert_eq!(output.len(), capacity);
    output
}

fn scan_serialized_map(bytes: &[u8]) -> Result<SerializedMapProfile, i32> {
    let count_bytes = bytes.get(..4).ok_or(STATUS_BAD_ARGUMENT)?;
    let pair_count = u32::from_le_bytes(count_bytes.try_into().map_err(|_| STATUS_BAD_ARGUMENT)?) as usize;
    let table_bytes = pair_count.checked_mul(8).ok_or(STATUS_INTERNAL_FAILURE)?;
    let table_end = 4usize.checked_add(table_bytes).ok_or(STATUS_INTERNAL_FAILURE)?;
    if table_end > bytes.len() {
        return Err(STATUS_BAD_ARGUMENT);
    }

    let mut data_offset = table_end;
    let mut lossy_string_bytes = 0usize;
    let mut path_normalization_bytes = 1usize;
    for index in 0..pair_count {
        let size_offset = 4usize
            .checked_add(index.checked_mul(8).ok_or(STATUS_INTERNAL_FAILURE)?)
            .ok_or(STATUS_INTERNAL_FAILURE)?;
        let key_len = u32::from_le_bytes(
            bytes[size_offset..size_offset + 4]
                .try_into()
                .map_err(|_| STATUS_BAD_ARGUMENT)?,
        ) as usize;
        let value_len = u32::from_le_bytes(
            bytes[size_offset + 4..size_offset + 8]
                .try_into()
                .map_err(|_| STATUS_BAD_ARGUMENT)?,
        ) as usize;
        let key_end = data_offset.checked_add(key_len).ok_or(STATUS_INTERNAL_FAILURE)?;
        let value_start = key_end.checked_add(1).ok_or(STATUS_INTERNAL_FAILURE)?;
        let value_end = value_start.checked_add(value_len).ok_or(STATUS_INTERNAL_FAILURE)?;
        let next = value_end.checked_add(1).ok_or(STATUS_INTERNAL_FAILURE)?;
        if next > bytes.len() {
            return Err(STATUS_BAD_ARGUMENT);
        }
        let key_lossy_bytes = lossy_utf8_output_len(&bytes[data_offset..key_end])?;
        let value_lossy_bytes = lossy_utf8_output_len(&bytes[value_start..value_end])?;
        lossy_string_bytes = lossy_string_bytes
            .checked_add(key_lossy_bytes)
            .and_then(|len| len.checked_add(value_lossy_bytes))
            .ok_or(STATUS_INTERNAL_FAILURE)?;
        if &bytes[data_offset..key_end] == b":path" {
            path_normalization_bytes = if bytes[value_start..value_end].starts_with(b"/") {
                0
            } else {
                value_lossy_bytes.checked_add(1).ok_or(STATUS_INTERNAL_FAILURE)?
            };
        }
        data_offset = next;
    }
    Ok(SerializedMapProfile {
        pair_count,
        lossy_string_bytes,
        path_normalization_bytes,
    })
}

pub fn decode_map(bytes: &[u8]) -> Option<Vec<(String, String)>> {
    let profile = scan_serialized_map(bytes).ok()?;
    let table_end = 4 + profile.pair_count * 8;
    let mut data_offset = table_end;
    let mut pairs = Vec::with_capacity(profile.pair_count);
    for index in 0..profile.pair_count {
        let size_offset = 4 + index * 8;
        let key_len = u32::from_le_bytes(bytes[size_offset..size_offset + 4].try_into().ok()?) as usize;
        let value_len = u32::from_le_bytes(bytes[size_offset + 4..size_offset + 8].try_into().ok()?) as usize;
        let key_end = data_offset + key_len;
        let value_start = key_end + 1;
        let value_end = value_start + value_len;
        let key_bytes = &bytes[data_offset..key_end];
        let value_bytes = &bytes[value_start..value_end];
        let key_capacity = lossy_utf8_output_len(key_bytes).ok()?;
        let value_capacity = lossy_utf8_output_len(value_bytes).ok()?;
        pairs.push((
            string_from_utf8_lossy_with_capacity(key_bytes, key_capacity),
            string_from_utf8_lossy_with_capacity(value_bytes, value_capacity),
        ));
        data_offset = value_end + 1;
    }
    Some(pairs)
}

/// Resolve a proxy-wasm buffer-type id to the host bytes it exposes.
/// `None` = unknown buffer type (caller returns `STATUS_NOT_FOUND`).
fn select_buffer(state: &HostState, buf_type: i32) -> Option<Vec<u8>> {
    match buf_type {
        BUF_VM_CONFIGURATION => Some(state.vm_config.clone()),
        BUF_PLUGIN_CONFIGURATION => Some(state.plugin_config.clone()),
        BUF_HTTP_CALL_RESPONSE_BODY => Some(
            state
                .callout_response
                .as_ref()
                .map(|r| r.body.clone())
                .unwrap_or_default(),
        ),
        // Buffer mode: the full request body staged by the body gate.
        BUF_HTTP_REQUEST_BODY => Some(state.request_body.clone()),
        // Read-only chunk window (body observation): the CURRENT chunk only,
        // never an accumulated body.
        BUF_HTTP_RESPONSE_BODY => Some(state.body_chunk.clone()),
        _ => None,
    }
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

/// Apply the proxy-wasm range-replacement contract to the staged request Body.
///
/// The range must be fully contained in the current buffer. `size == 0`
/// inserts, `start == len && size == 0` appends, an empty replacement deletes,
/// and all other calls replace exactly `size` bytes. The physical output cap
/// is checked before `splice`, so every rejected operation is atomic.
fn request_body_replacement_range(
    state: &HostState,
    start: usize,
    size: usize,
    replacement_len: usize,
) -> Result<std::ops::Range<usize>, i32> {
    if state.phase != WasmPhase::RequestBody {
        return Err(STATUS_BAD_ARGUMENT);
    }
    let end = start.checked_add(size).ok_or(STATUS_BAD_ARGUMENT)?;
    if start > state.request_body.len() || end > state.request_body.len() {
        return Err(STATUS_BAD_ARGUMENT);
    }
    let new_len = state
        .request_body
        .len()
        .checked_sub(size)
        .and_then(|len| len.checked_add(replacement_len))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    if new_len > state.request_body_limit {
        return Err(STATUS_INTERNAL_FAILURE);
    }
    Ok(start..end)
}

fn request_body_mutation_work(current_len: usize, range: &std::ops::Range<usize>, replacement_len: usize) -> usize {
    let shifted_tail = if replacement_len == range.len() {
        0
    } else {
        current_len.saturating_sub(range.end)
    };
    replacement_len.saturating_add(shifted_tail)
}

fn request_body_mutation_work_limit(physical_cap: usize) -> usize {
    physical_cap.saturating_mul(MAX_REQUEST_BODY_MUTATION_WORK_PASSES)
}

fn checked_request_body_mutation_total(
    current_work: usize,
    added_work: usize,
    physical_cap: usize,
) -> Result<usize, i32> {
    let new_work = current_work.checked_add(added_work).ok_or(STATUS_INTERNAL_FAILURE)?;
    if new_work > request_body_mutation_work_limit(physical_cap) {
        return Err(STATUS_INTERNAL_FAILURE);
    }
    Ok(new_work)
}

fn replace_request_body_buffer(state: &mut HostState, start: usize, size: usize, replacement: Vec<u8>) -> i32 {
    let range = match request_body_replacement_range(state, start, size, replacement.len()) {
        Ok(range) => range,
        Err(status) => return status,
    };
    let work = request_body_mutation_work(state.request_body.len(), &range, replacement.len());
    if let Err(status) = state.charge_request_body_mutation(work) {
        return status;
    }
    if range.is_empty() && replacement.is_empty() {
        return STATUS_OK;
    }
    state.request_body.splice(range, replacement);
    state.request_body_modified = true;
    STATUS_OK
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

fn guest_range(ptr: i32, len: i32, memory_size: usize) -> Result<std::ops::Range<usize>, i32> {
    if ptr < 0 || len < 0 {
        return Err(STATUS_BAD_ARGUMENT);
    }
    let start = ptr as usize;
    let end = start
        .checked_add(len as usize)
        .filter(|end| *end <= memory_size)
        .ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    Ok(start..end)
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

fn callout_input_payload_bytes(upstream_size: i32, headers_size: i32, body_size: i32) -> Result<usize, i32> {
    let upstream = usize::try_from(upstream_size).map_err(|_| STATUS_BAD_ARGUMENT)?;
    let headers = usize::try_from(headers_size).map_err(|_| STATUS_BAD_ARGUMENT)?;
    // Preserve the existing ABI behavior: a non-positive body size means an
    // empty body and does not dereference `body_ptr`.
    let body = usize::try_from(body_size.max(0)).map_err(|_| STATUS_BAD_ARGUMENT)?;
    upstream
        .checked_add(headers)
        .and_then(|n| n.checked_add(body))
        .ok_or(STATUS_BAD_ARGUMENT)
}

fn callout_allocation_preflight_payload(
    upstream_bytes: &[u8],
    header_bytes: &[u8],
    body_size: usize,
) -> Result<usize, i32> {
    let profile = scan_serialized_map(header_bytes)?;
    let raw_guest_copies = upstream_bytes
        .len()
        .checked_add(header_bytes.len())
        .and_then(|len| len.checked_add(body_size))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    let lossy_owned_bytes = lossy_utf8_output_len(upstream_bytes)?
        .checked_add(profile.lossy_string_bytes)
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    // `decode_map`'s Vec is consumed while the filtered PendingCallout header
    // Vec is built, so both pair allocations coexist briefly.
    let pair_vec_bytes = profile
        .pair_count
        .checked_mul(std::mem::size_of::<(String, String)>())
        .and_then(|bytes| bytes.checked_mul(2))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    // `url::Url` owns a normalized serialization while parsing. Percent
    // encoding expands one byte to at most three ASCII bytes; the fourth
    // factor plus fixed slack covers IDNA's `xn--` label prefix and URL
    // delimiters. The returned normalized origin is a second concurrently
    // live String of no greater size.
    let upstream_lossy_bytes = lossy_utf8_output_len(upstream_bytes)?;
    let one_origin_representation = upstream_lossy_bytes
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(64))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    let origin_bytes = one_origin_representation
        .checked_mul(2)
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    // The default method/path Strings are allocated before pseudo headers are
    // consumed. A non-slash :path is rebuilt into an exactly pre-sized String
    // instead of `insert(0)`, whose growth policy could temporarily allocate
    // an unaccounted larger buffer.
    let callout_fixed_bytes = std::mem::size_of::<PendingCallout>()
        .checked_add(4)
        .and_then(|bytes| bytes.checked_add(profile.path_normalization_bytes))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    raw_guest_copies
        .checked_add(lossy_owned_bytes)
        .and_then(|bytes| bytes.checked_add(pair_vec_bytes))
        .and_then(|bytes| bytes.checked_add(origin_bytes))
        .and_then(|bytes| bytes.checked_add(callout_fixed_bytes))
        .ok_or(STATUS_INTERNAL_FAILURE)
}

fn pending_callout_retained_payload(callout: &PendingCallout) -> Result<usize, i32> {
    let mut payload = std::mem::size_of::<PendingCallout>()
        .checked_add(callout.origin.capacity())
        .and_then(|n| n.checked_add(callout.path.capacity()))
        .and_then(|n| n.checked_add(callout.method.capacity()))
        .and_then(|n| n.checked_add(callout.authority.as_ref().map_or(0, String::capacity)))
        .and_then(|n| n.checked_add(callout.body.capacity()))
        .and_then(|n| {
            callout
                .headers
                .capacity()
                .checked_mul(std::mem::size_of::<(String, String)>())
                .and_then(|overhead| n.checked_add(overhead))
        })
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    for (name, value) in &callout.headers {
        payload = payload
            .checked_add(name.capacity())
            .and_then(|n| n.checked_add(value.capacity()))
            .ok_or(STATUS_INTERNAL_FAILURE)?;
    }
    Ok(payload)
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
    if caller.data().callouts_dispatched >= MAX_CALLOUTS_PER_REQUEST {
        crate::observe::metrics::registry::record_wasm_callout(&caller.data().metrics_label, "over_limit");
        return STATUS_INTERNAL_FAILURE;
    }
    let input_payload = match callout_input_payload_bytes(upstream_size, headers_size, body_size) {
        Ok(payload) => payload,
        Err(status) => return status,
    };
    // Reject from guest-provided lengths before copying any large buffer. This
    // is only a preflight; the exact final owned representation is checked
    // again after normalization/decoding and committed atomically at the end.
    if let Err(status) = caller.data().checked_abi_charge(input_payload) {
        return status;
    }
    let body_len = body_size.max(0) as usize;
    let allocation_preflight = {
        let Some(memory) = guest_memory(&mut caller) else {
            return STATUS_INVALID_MEMORY_ACCESS;
        };
        let memory_bytes = memory.data(&caller);
        let upstream_range = match guest_range(upstream_ptr, upstream_size, memory_bytes.len()) {
            Ok(range) => range,
            Err(status) => return status,
        };
        let headers_range = match guest_range(headers_ptr, headers_size, memory_bytes.len()) {
            Ok(range) => range,
            Err(status) => return status,
        };
        if body_size > 0 {
            if let Err(status) = guest_range(body_ptr, body_size, memory_bytes.len()) {
                return status;
            }
        }
        match callout_allocation_preflight_payload(
            &memory_bytes[upstream_range],
            &memory_bytes[headers_range],
            body_len,
        ) {
            Ok(payload) => payload,
            Err(status) => return status,
        }
    };
    if let Err(status) = caller.data().checked_abi_charge(allocation_preflight) {
        return status;
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
    drop(header_bytes);
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
    let origin = match edgion_resources::plugin_configs::normalize_callout_origin_quiet(&upstream) {
        Some(origin) => origin,
        // Data-plane hot path (Iron rule 3), guest-supplied origin (Iron rule 1):
        // reject via status code only, no tracing or raw-bearing error String.
        None => return STATUS_BAD_ARGUMENT,
    };

    let state = caller.data_mut();
    if !state.callout_allowlist.iter().any(|a| a == &origin) {
        // Bounded metric carries the signal; the guest-supplied origin must not
        // reach tracing on the request path.
        crate::observe::metrics::registry::record_wasm_callout(&state.metrics_label, "denied");
        return STATUS_INTERNAL_FAILURE;
    }
    let mut method = None;
    let mut path = None;
    let mut authority = None;
    let mut headers = Vec::with_capacity(all_headers.len());
    for (name, value) in all_headers {
        match name.as_str() {
            ":method" => method = Some(value),
            ":path" => path = Some(value),
            ":authority" => authority = Some(value),
            n if n.starts_with(':') => {}
            _ => headers.push((name, value)),
        }
    }
    let method = method.unwrap_or_else(|| "GET".to_string());
    let path = match path {
        Some(path) if path.starts_with('/') => path,
        Some(path) => {
            let Some(capacity) = path.len().checked_add(1) else {
                return STATUS_INTERNAL_FAILURE;
            };
            let mut normalized = String::with_capacity(capacity);
            normalized.push('/');
            normalized.push_str(&path);
            normalized
        }
        None => "/".to_string(),
    };

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
    let retained_payload = match pending_callout_retained_payload(&pending) {
        Ok(payload) => payload,
        Err(status) => return status,
    };
    let checked_charge = match state.checked_abi_charge(retained_payload) {
        Ok(charge) => charge,
        Err(status) => return status,
    };

    // Deliver the token to the guest FIRST; only commit the dispatch (increment
    // counter + queue + retained budget) if the write succeeds. A bad
    // return_token_ptr must not leave any partial host-side state.
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
    state.commit_abi_charge(checked_charge);
    state.callouts_dispatched += 1;
    state.next_callout_token = token.wrapping_add(1);
    state.pending_callouts.push(pending);
    STATUS_OK
}

/// Register the implemented proxy-wasm host functions. Compatibility analysis
/// rejects unknown imports before compilation; the caller may still define
/// unknown imports as traps after this as defense in depth.
pub fn add_to_linker(linker: &mut Linker<HostState>) -> anyhow::Result<()> {
    linker.func_wrap(
        host::MODULE,
        host::PROXY_LOG,
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
        host::MODULE,
        host::PROXY_GET_CURRENT_TIME_NANOSECONDS,
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
        host::MODULE,
        host::PROXY_GET_HEADER_MAP_PAIRS,
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
        host::MODULE,
        host::PROXY_GET_HEADER_MAP_VALUE,
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
        host::MODULE,
        host::PROXY_REPLACE_HEADER_MAP_VALUE,
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
        host::MODULE,
        host::PROXY_ADD_HEADER_MAP_VALUE,
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
        host::MODULE,
        host::PROXY_REMOVE_HEADER_MAP_VALUE,
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
        host::MODULE,
        host::PROXY_SET_HEADER_MAP_PAIRS,
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
        host::MODULE,
        host::PROXY_GET_BUFFER_BYTES,
        |mut caller: Caller<'_, HostState>,
         buf_type: i32,
         start: i32,
         max_size: i32,
         ret_ptr: i32,
         ret_size: i32|
         -> i32 {
            let Some(data) = select_buffer(caller.data(), buf_type) else {
                return STATUS_NOT_FOUND;
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
        host::MODULE,
        host::PROXY_SET_BUFFER_BYTES,
        |mut caller: Caller<'_, HostState>, buf: i32, start: i32, size: i32, ptr: i32, len: i32| -> i32 {
            if buf == BUF_HTTP_RESPONSE_BODY {
                // Proxied upstream responses remain streaming-only and
                // read-only. Keep this a defined ABI error rather than a trap.
                return STATUS_UNIMPLEMENTED;
            }
            if buf != BUF_HTTP_REQUEST_BODY {
                return if select_buffer(caller.data(), buf).is_some() {
                    STATUS_BAD_ARGUMENT
                } else {
                    STATUS_NOT_FOUND
                };
            }
            if caller.data().phase != WasmPhase::RequestBody {
                return STATUS_BAD_ARGUMENT;
            }
            if len < 0 {
                return STATUS_BAD_ARGUMENT;
            }
            let start = start as u32 as usize;
            let size = size as u32 as usize;
            // Validate the range and final output length before copying from
            // guest memory. Any operation that can succeed is now bounded by
            // the physical Body cap, so a malicious guest cannot force a
            // large allocation for an obviously impossible mutation.
            let range = match request_body_replacement_range(caller.data(), start, size, len as usize) {
                Ok(range) => range,
                Err(status) => return status,
            };
            let work = request_body_mutation_work(caller.data().request_body.len(), &range, len as usize);
            if let Err(status) = caller.data().check_request_body_mutation_charge(work) {
                return status;
            }
            let replacement = match read_guest(&mut caller, ptr, len) {
                Ok(bytes) => bytes,
                Err(status) => return status,
            };
            replace_request_body_buffer(caller.data_mut(), start, size, replacement)
        },
    )?;

    linker.func_wrap(
        host::MODULE,
        host::PROXY_SEND_LOCAL_RESPONSE,
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
        host::MODULE,
        host::PROXY_GET_PROPERTY,
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
        host::MODULE,
        host::PROXY_SET_PROPERTY,
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
        host::MODULE,
        host::PROXY_GET_CONFIGURATION,
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
    linker.func_wrap(host::MODULE, host::PROXY_HTTP_CALL, host_http_call)?;
    linker.func_wrap(host::MODULE, host::PROXY_DISPATCH_HTTP_CALL, host_http_call)?;

    // Shared data (per-module KV with CAS). Process-local: shared across
    // all pooled VMs of one ModuleEntry, freed with the cache entry.
    linker.func_wrap(
        host::MODULE,
        host::PROXY_GET_SHARED_DATA,
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
        host::MODULE,
        host::PROXY_SET_SHARED_DATA,
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
        host::MODULE,
        host::PROXY_SET_EFFECTIVE_CONTEXT,
        |_: Caller<'_, HostState>, _ctx: i32| -> i32 { STATUS_OK },
    )?;
    linker.func_wrap(
        host::MODULE,
        host::PROXY_SET_TICK_PERIOD_MILLISECONDS,
        |mut caller: Caller<'_, HostState>, ms: i32| -> i32 {
            caller.data_mut().tick_period_ms = ms.max(0) as u64;
            STATUS_OK
        },
    )?;
    linker.func_wrap(host::MODULE, host::PROXY_DONE, |_: Caller<'_, HostState>| -> i32 {
        STATUS_OK
    })?;
    // Pause/resume: the guest resumes a paused request either through the
    // 0.1-style proxy_continue_request or the 0.2-style
    // proxy_continue_stream(stream_type=0 /* Request */).
    linker.func_wrap(
        host::MODULE,
        host::PROXY_CONTINUE_STREAM,
        |mut caller: Caller<'_, HostState>, stream: i32| -> i32 {
            if stream == 0 {
                caller.data_mut().continue_requested = true;
            }
            STATUS_OK
        },
    )?;
    linker.func_wrap(
        host::MODULE,
        host::PROXY_CONTINUE_REQUEST,
        |mut caller: Caller<'_, HostState>| -> i32 {
            caller.data_mut().continue_requested = true;
            STATUS_OK
        },
    )?;
    linker.func_wrap(
        host::MODULE,
        host::PROXY_CONTINUE_RESPONSE,
        |_: Caller<'_, HostState>| -> i32 { STATUS_OK },
    )?;

    for function in host::FUNCTIONS
        .iter()
        .filter(|function| function.behavior == HostBehavior::Trap)
    {
        let ty = FuncType::new(
            linker.engine(),
            function.signature.params.iter().copied().map(linker_value_type),
            function.signature.results.iter().copied().map(linker_value_type),
        );
        linker.func_new(function.module, function.name, ty, |_, _, _| {
            Err(wasmtime::Error::msg("unsupported proxy-wasm host function"))
        })?;
    }

    Ok(())
}

fn linker_value_type(value: WasmValueType) -> ValType {
    match value {
        WasmValueType::I32 => ValType::I32,
        WasmValueType::I64 => ValType::I64,
        WasmValueType::F32 => ValType::F32,
        WasmValueType::F64 => ValType::F64,
        WasmValueType::V128 => ValType::V128,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::http::wasm::shared_data::SharedData;
    use edgion_wasm::HostFunction;
    use wasmtime::{Engine, Instance, Module, Store};

    fn profile_value_type(value: wasmtime::ValType) -> WasmValueType {
        match value {
            wasmtime::ValType::I32 => WasmValueType::I32,
            wasmtime::ValType::I64 => WasmValueType::I64,
            wasmtime::ValType::F32 => WasmValueType::F32,
            wasmtime::ValType::F64 => WasmValueType::F64,
            wasmtime::ValType::V128 => WasmValueType::V128,
            other => panic!("host linker unexpectedly uses reference type {other}"),
        }
    }

    #[test]
    fn host_profile_matches_linker_names_and_signatures() {
        let engine = Engine::default();
        let mut linker = Linker::new(&engine);
        add_to_linker(&mut linker).expect("host linker registration");
        let state = HostState::new(
            Vec::new(),
            Vec::new(),
            "contract-test".to_string(),
            Arc::new(SharedData::new("contract-test".to_string())),
        );
        let mut store = Store::new(&engine, state);
        let linker_items: Vec<(String, String, Extern)> = linker
            .iter(&mut store)
            .map(|(module, name, item)| (module.to_string(), name.to_string(), item))
            .collect();
        let mut linked: Vec<HostFunction> = linker_items
            .into_iter()
            .map(|(module, name, item)| {
                let Extern::Func(function) = item else {
                    panic!("host linker item {module}::{name} must be a function");
                };
                let ty = function.ty(&store);
                let params: Vec<WasmValueType> = ty.params().map(profile_value_type).collect();
                let results: Vec<WasmValueType> = ty.results().map(profile_value_type).collect();
                let profile = host::FUNCTIONS
                    .iter()
                    .find(|profile| profile.module == module && profile.name == name)
                    .unwrap_or_else(|| panic!("linker function {module}::{name} is absent from the host profile"));
                assert_eq!(params, profile.signature.params, "parameter drift for {module}::{name}");
                assert_eq!(results, profile.signature.results, "result drift for {module}::{name}");
                *profile
            })
            .collect();
        linked.sort_by_key(|function| (function.module, function.name));
        let mut profiled = host::FUNCTIONS.to_vec();
        profiled.sort_by_key(|function| (function.module, function.name));
        assert_eq!(linked, profiled, "host profile and linker name sets differ");
    }

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

    fn encode_raw_map(pairs: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
        for (key, value) in pairs {
            encoded.extend_from_slice(&(key.len() as u32).to_le_bytes());
            encoded.extend_from_slice(&(value.len() as u32).to_le_bytes());
        }
        for (key, value) in pairs {
            encoded.extend_from_slice(key);
            encoded.push(0);
            encoded.extend_from_slice(value);
            encoded.push(0);
        }
        encoded
    }

    #[test]
    fn decode_map_preserves_lossy_utf8_compatibility() {
        let invalid_key = [b'x', 0xFF, b'y'];
        let invalid_value = [0xF0, 0x80, 0x80, 0x80];
        let encoded = encode_raw_map(&[(&invalid_key, &invalid_value)]);
        let decoded = decode_map(&encoded).unwrap();

        assert_eq!(
            decoded,
            vec![(
                String::from_utf8_lossy(&invalid_key).into_owned(),
                String::from_utf8_lossy(&invalid_value).into_owned(),
            )]
        );
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

    const CALLOUT_BUDGET_GUEST: &str = r#"
    (module
      (import "env" "proxy_dispatch_http_call"
        (func $call (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "http://example.com")
      ;; encoded empty header map
      (data (i32.const 32) "\00\00\00\00")
      ;; malformed map: count=0xffffffff but only one size-table entry
      (data (i32.const 64) "\ff\ff\ff\ff\00\00\00\00\00\00\00\00")
      (func (export "dispatch") (param $token_ptr i32) (result i32)
        (call $call
          (i32.const 0) (i32.const 18)
          (i32.const 32) (i32.const 4)
          (i32.const 0) (i32.const 0)
          (i32.const 0) (i32.const 0)
          (i32.const 1000) (local.get $token_ptr)))
      (func (export "dispatch_bad_map") (param $token_ptr i32) (result i32)
        (call $call
          (i32.const 0) (i32.const 18)
          (i32.const 64) (i32.const 12)
          (i32.const 0) (i32.const 0)
          (i32.const 0) (i32.const 0)
          (i32.const 1000) (local.get $token_ptr)))
      (func (export "dispatch_custom_headers")
        (param $headers_ptr i32) (param $headers_size i32) (param $token_ptr i32)
        (result i32)
        (call $call
          (i32.const 0) (i32.const 18)
          (local.get $headers_ptr) (local.get $headers_size)
          (i32.const 0) (i32.const 0)
          (i32.const 0) (i32.const 0)
          (i32.const 1000) (local.get $token_ptr))))
    "#;

    fn instantiate_callout_guest() -> (Store<HostState>, Instance) {
        let engine = Engine::default();
        let module = Module::new(&engine, CALLOUT_BUDGET_GUEST).expect("callout guest compiles");
        let mut linker: Linker<HostState> = Linker::new(&engine);
        add_to_linker(&mut linker).unwrap();
        let mut store = Store::new(&engine, HostState::default());
        let instance = linker.instantiate(&mut store, &module).unwrap();
        store
            .data_mut()
            .begin_phase(WasmPhase::RequestHeaders, Vec::new(), Vec::new(), HashMap::new());
        store
            .data_mut()
            .arm_dispatch(Arc::from(vec!["http://example.com:80".to_string()]), 1000);
        (store, instance)
    }

    #[test]
    fn callout_budget_rejects_next_dispatch_without_partial_state() {
        let (mut store, instance) = instantiate_callout_guest();
        let dispatch = instance.get_typed_func::<i32, i32>(&mut store, "dispatch").unwrap();
        assert_eq!(dispatch.call(&mut store, 124).unwrap(), STATUS_OK);
        let callout_cost = store.data().retained_bytes;
        store
            .data_mut()
            .begin_phase(WasmPhase::RequestHeaders, Vec::new(), Vec::new(), HashMap::new());
        store
            .data_mut()
            .arm_dispatch(Arc::from(vec!["http://example.com:80".to_string()]), 1000);
        let preflight_cost = callout_allocation_preflight_payload(b"http://example.com", &[0, 0, 0, 0], 0).unwrap()
            + ABI_OP_OVERHEAD_BYTES;
        let admission_cost = callout_cost.max(preflight_cost);
        let retained_before = MAX_RETAINED_ABI_BYTES - admission_cost;
        store.data_mut().retained_bytes = retained_before;

        assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_OK);
        let state = store.data();
        assert_eq!(state.retained_bytes, retained_before + callout_cost);
        assert_eq!(state.retained_ops, 1);
        assert_eq!(state.pending_callouts.len(), 1);
        assert_eq!(state.callouts_dispatched, 1);
        assert_eq!(state.next_callout_token, 2);

        assert_eq!(dispatch.call(&mut store, 132).unwrap(), STATUS_INTERNAL_FAILURE);
        let state = store.data();
        assert_eq!(state.retained_bytes, retained_before + callout_cost);
        assert_eq!(state.retained_ops, 1);
        assert_eq!(state.pending_callouts.len(), 1);
        assert_eq!(state.callouts_dispatched, 1);
        assert_eq!(state.next_callout_token, 2);
    }

    #[test]
    fn malformed_callout_map_count_is_bad_argument_without_state_change() {
        let (mut store, instance) = instantiate_callout_guest();
        let dispatch = instance
            .get_typed_func::<i32, i32>(&mut store, "dispatch_bad_map")
            .unwrap();
        assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_BAD_ARGUMENT);
        let state = store.data();
        assert_eq!(state.retained_bytes, 0);
        assert_eq!(state.retained_ops, 0);
        assert!(state.pending_callouts.is_empty());
        assert_eq!(state.callouts_dispatched, 0);
        assert_eq!(state.next_callout_token, 1);
    }

    #[test]
    fn final_callout_capacity_charge_rejects_atomically_after_raw_preflight() {
        let (mut store, instance) = instantiate_callout_guest();
        let dispatch = instance.get_typed_func::<i32, i32>(&mut store, "dispatch").unwrap();
        let raw_preflight_cost = 18 + 4 + ABI_OP_OVERHEAD_BYTES;
        let retained_before = MAX_RETAINED_ABI_BYTES - raw_preflight_cost;
        store.data_mut().retained_bytes = retained_before;
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        memory.write(&mut store, 128, &[0xA5; 4]).unwrap();

        assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_INTERNAL_FAILURE);
        let mut token_bytes = [0u8; 4];
        memory.read(&store, 128, &mut token_bytes).unwrap();
        assert_eq!(token_bytes, [0xA5; 4]);
        let state = store.data();
        assert_eq!(state.retained_bytes, retained_before);
        assert_eq!(state.retained_ops, 0);
        assert!(state.pending_callouts.is_empty());
        assert_eq!(state.callouts_dispatched, 0);
        assert_eq!(state.next_callout_token, 1);
    }

    #[test]
    fn invalid_callout_token_pointer_commits_nothing() {
        let (mut store, instance) = instantiate_callout_guest();
        let dispatch = instance.get_typed_func::<i32, i32>(&mut store, "dispatch").unwrap();
        assert_eq!(
            dispatch.call(&mut store, i32::MAX).unwrap(),
            STATUS_INVALID_MEMORY_ACCESS
        );
        let state = store.data();
        assert_eq!(state.retained_bytes, 0);
        assert_eq!(state.retained_ops, 0);
        assert!(state.pending_callouts.is_empty());
        assert_eq!(state.callouts_dispatched, 0);
        assert_eq!(state.next_callout_token, 1);
    }

    #[test]
    fn callout_budget_precheck_and_phase_reset_use_canonical_budget() {
        let mut state = HostState::default();
        let payload = 10;
        let per_callout = payload + ABI_OP_OVERHEAD_BYTES;
        for _ in 0..2 {
            let checked = state
                .checked_abi_charge_with_limits(payload, per_callout * 2, 2)
                .expect("callout fits small test cap");
            state.commit_abi_charge(checked);
        }
        assert_eq!(
            state.checked_abi_charge_with_limits(payload, per_callout * 2, 2),
            Err(STATUS_INTERNAL_FAILURE)
        );

        state.retained_bytes = MAX_RETAINED_ABI_BYTES;
        let large_input = callout_input_payload_bytes(i32::MAX, i32::MAX, i32::MAX).unwrap();
        assert_eq!(
            state.checked_abi_charge(large_input),
            Err(STATUS_INTERNAL_FAILURE),
            "length preflight rejects before any guest-memory copy"
        );

        state.begin_phase(WasmPhase::Tick, Vec::new(), Vec::new(), HashMap::new());
        assert_eq!(state.retained_bytes, 0);
        assert_eq!(state.retained_ops, 0);
    }

    #[test]
    fn callout_preflight_rejects_lossy_utf8_expansion_without_retained_state_change() {
        let upstream = b"http://example.com";
        let invalid_name = [0xFF; 16];
        let headers = encode_raw_map(&[(&invalid_name, b"value")]);
        let raw_payload = upstream.len() + headers.len();
        let allocation_payload = callout_allocation_preflight_payload(upstream, &headers, 0).unwrap();
        let valid_name = [b'x'; 16];
        let valid_headers = encode_raw_map(&[(&valid_name, b"value")]);
        let valid_allocation_payload = callout_allocation_preflight_payload(upstream, &valid_headers, 0).unwrap();
        assert_eq!(
            allocation_payload - valid_allocation_payload,
            invalid_name.len() * 2,
            "each invalid byte expands from one raw byte to a three-byte replacement"
        );

        let state = HostState {
            retained_bytes: MAX_RETAINED_ABI_BYTES - raw_payload - ABI_OP_OVERHEAD_BYTES,
            ..Default::default()
        };
        let retained_before = state.retained_bytes;
        assert!(state.checked_abi_charge(raw_payload).is_ok());
        assert_eq!(
            state.checked_abi_charge(allocation_payload),
            Err(STATUS_INTERNAL_FAILURE)
        );
        assert_eq!(state.retained_bytes, retained_before);
        assert_eq!(state.retained_ops, 0);
        assert!(state.pending_callouts.is_empty());
    }

    #[test]
    fn high_pair_count_is_rejected_on_real_host_call_before_token_or_state_commit() {
        let (mut store, instance) = instantiate_callout_guest();
        let dispatch = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "dispatch_custom_headers")
            .unwrap();
        let empty_pairs = vec![(&[][..], &[][..]); 128];
        let headers = encode_raw_map(&empty_pairs);
        let headers_ptr = 1024;
        let token_ptr = 512;
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        memory.write(&mut store, headers_ptr, &headers).unwrap();
        memory.write(&mut store, token_ptr, &[0xA5; 4]).unwrap();

        let raw_payload = b"http://example.com".len() + headers.len();
        let retained_before = MAX_RETAINED_ABI_BYTES - raw_payload - ABI_OP_OVERHEAD_BYTES;
        store.data_mut().retained_bytes = retained_before;
        assert_eq!(
            dispatch
                .call(&mut store, (headers_ptr as i32, headers.len() as i32, token_ptr as i32),)
                .unwrap(),
            STATUS_INTERNAL_FAILURE
        );

        let mut token = [0u8; 4];
        memory.read(&store, token_ptr, &mut token).unwrap();
        assert_eq!(token, [0xA5; 4]);
        let state = store.data();
        assert_eq!(state.retained_bytes, retained_before);
        assert_eq!(state.retained_ops, 0);
        assert!(state.pending_callouts.is_empty());
        assert_eq!(state.callouts_dispatched, 0);
        assert_eq!(state.next_callout_token, 1);
    }

    #[test]
    fn callout_preflight_rejects_high_pair_count_without_retained_state_change() {
        let upstream = b"http://example.com";
        let empty_pairs = vec![(&[][..], &[][..]); 128];
        let headers = encode_raw_map(&empty_pairs);
        let raw_payload = upstream.len() + headers.len();
        let allocation_payload = callout_allocation_preflight_payload(upstream, &headers, 0).unwrap();
        assert!(
            allocation_payload >= raw_payload + 128 * 2 * std::mem::size_of::<(String, String)>(),
            "both concurrently-live pair Vec capacities must be preflighted"
        );

        let state = HostState {
            retained_bytes: MAX_RETAINED_ABI_BYTES - raw_payload - ABI_OP_OVERHEAD_BYTES,
            ..Default::default()
        };
        let retained_before = state.retained_bytes;
        assert!(state.checked_abi_charge(raw_payload).is_ok());
        assert_eq!(
            state.checked_abi_charge(allocation_payload),
            Err(STATUS_INTERNAL_FAILURE)
        );
        assert_eq!(state.retained_bytes, retained_before);
        assert_eq!(state.retained_ops, 0);
        assert!(state.pending_callouts.is_empty());
    }

    /// `begin_phase` must clear the staged request body between phases so a
    /// stale buffer-mode body never leaks into a later phase's `HostState`.
    #[test]
    fn begin_phase_clears_request_body() {
        let mut state = HostState::default();
        state.request_body = vec![1, 2, 3];
        state.request_body_limit = 9;
        state.request_body_modified = true;
        state.request_body_mutation_work = 123;
        state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
        assert!(state.request_body.is_empty());
        assert_eq!(state.request_body_limit, 0);
        assert!(!state.request_body_modified);
        assert_eq!(state.request_body_mutation_work, 0);
        assert_eq!(state.phase, WasmPhase::RequestBody);
    }

    /// `select_buffer` resolves `BUF_HTTP_REQUEST_BODY` to the staged full
    /// request body, keeps the existing response-body chunk-window behavior,
    /// and returns `None` for an unknown buffer-type id.
    #[test]
    fn buffer_select_returns_request_body() {
        let mut state = HostState::default();
        state.request_body = b"POST-BODY".to_vec();
        assert_eq!(
            select_buffer(&state, BUF_HTTP_REQUEST_BODY),
            Some(b"POST-BODY".to_vec())
        );
        assert_eq!(select_buffer(&state, BUF_HTTP_RESPONSE_BODY), Some(Vec::new()));
        assert_eq!(select_buffer(&state, 999), None);
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

    #[test]
    fn request_body_range_mutations_are_atomic_and_bounded() {
        let mut state = HostState::default();
        state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
        state.request_body = b"abcdef".to_vec();
        state.request_body_limit = 12;

        assert_eq!(replace_request_body_buffer(&mut state, 0, 0, Vec::new()), STATUS_OK);
        assert!(!state.request_body_modified, "an empty insertion is a no-op");
        assert_eq!(
            replace_request_body_buffer(&mut state, 1, 2, b"XYZ".to_vec()),
            STATUS_OK
        );
        assert_eq!(state.request_body, b"aXYZdef");
        assert_eq!(replace_request_body_buffer(&mut state, 1, 0, b"+".to_vec()), STATUS_OK);
        assert_eq!(state.request_body, b"a+XYZdef");
        let end = state.request_body.len();
        assert_eq!(
            replace_request_body_buffer(&mut state, end, 0, b"!".to_vec()),
            STATUS_OK
        );
        assert_eq!(state.request_body, b"a+XYZdef!");
        assert_eq!(replace_request_body_buffer(&mut state, 2, 3, Vec::new()), STATUS_OK);
        assert_eq!(state.request_body, b"a+def!");

        let before = state.request_body.clone();
        assert_eq!(
            replace_request_body_buffer(&mut state, before.len() + 1, 0, b"x".to_vec()),
            STATUS_BAD_ARGUMENT
        );
        assert_eq!(state.request_body, before);
        assert_eq!(
            replace_request_body_buffer(&mut state, 0, 0, vec![b'x'; 20]),
            STATUS_INTERNAL_FAILURE
        );
        assert_eq!(state.request_body, before);

        // Body replacement payload is not append-only retained ABI memory, so
        // it uses the separate mutation-work counter. Even near the generic
        // retained-byte budget, one equal-size rewrite succeeds.
        state.retained_bytes = MAX_RETAINED_ABI_BYTES - ABI_OP_OVERHEAD_BYTES;
        let replacement = vec![b'z'; state.request_body.len()];
        let body_len = state.request_body.len();
        let work_before = state.request_body_mutation_work;
        assert_eq!(
            replace_request_body_buffer(&mut state, 0, body_len, replacement),
            STATUS_OK
        );
        assert_eq!(state.retained_bytes, MAX_RETAINED_ABI_BYTES);
        assert_eq!(state.request_body_mutation_work, work_before + body_len);
        assert!(state.request_body_modified);
    }

    #[test]
    fn request_body_repeated_full_rewrites_stop_at_work_budget_atomically() {
        let mut state = HostState::default();
        state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
        state.request_body = b"abcd".to_vec();
        state.request_body_limit = 4;
        state.request_body_mutation_work = request_body_mutation_work_limit(state.request_body_limit) - 4;

        assert_eq!(
            replace_request_body_buffer(&mut state, 0, 4, b"WXYZ".to_vec()),
            STATUS_OK
        );
        assert_eq!(state.request_body, b"WXYZ");
        assert_eq!(
            state.request_body_mutation_work,
            request_body_mutation_work_limit(state.request_body_limit)
        );

        let retained_bytes = state.retained_bytes;
        let retained_ops = state.retained_ops;
        assert_eq!(
            replace_request_body_buffer(&mut state, 0, 4, b"nope".to_vec()),
            STATUS_INTERNAL_FAILURE
        );
        assert_eq!(state.request_body, b"WXYZ");
        assert_eq!(
            state.request_body_mutation_work,
            request_body_mutation_work_limit(state.request_body_limit)
        );
        assert_eq!(state.retained_bytes, retained_bytes);
        assert_eq!(state.retained_ops, retained_ops);
    }

    #[test]
    fn request_body_length_change_charges_shifted_tail_work() {
        let mut state = HostState::default();
        state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
        state.request_body = b"abcd".to_vec();
        state.request_body_limit = 4;
        state.request_body_mutation_work = request_body_mutation_work_limit(state.request_body_limit) - 3;

        // Deleting the first byte moves the remaining three-byte tail.
        assert_eq!(replace_request_body_buffer(&mut state, 0, 1, Vec::new()), STATUS_OK);
        assert_eq!(state.request_body, b"bcd");
        assert_eq!(
            state.request_body_mutation_work,
            request_body_mutation_work_limit(state.request_body_limit)
        );

        // A second front deletion would move another two-byte tail and must
        // be rejected without changing either the body or the counters.
        let retained_bytes = state.retained_bytes;
        let retained_ops = state.retained_ops;
        assert_eq!(
            replace_request_body_buffer(&mut state, 0, 1, Vec::new()),
            STATUS_INTERNAL_FAILURE
        );
        assert_eq!(state.request_body, b"bcd");
        assert_eq!(state.retained_bytes, retained_bytes);
        assert_eq!(state.retained_ops, retained_ops);
    }

    #[test]
    fn request_body_mutation_budget_scales_with_physical_cap_without_overflow() {
        const MIB: usize = 1024 * 1024;
        let large_cap = 96 * MIB;
        assert_eq!(request_body_mutation_work_limit(large_cap), 192 * MIB);
        assert_eq!(
            checked_request_body_mutation_total(0, large_cap, large_cap),
            Ok(large_cap),
            "a first equal-size rewrite at a 96 MiB physical cap must fit"
        );
        assert_eq!(
            checked_request_body_mutation_total(large_cap, large_cap, large_cap),
            Ok(192 * MIB)
        );
        assert_eq!(
            checked_request_body_mutation_total(192 * MIB, 1, large_cap),
            Err(STATUS_INTERNAL_FAILURE),
            "repeated mutation must stop at two physical-cap passes"
        );

        let small_cap = 4 * 1024;
        assert_eq!(request_body_mutation_work_limit(small_cap), 8 * 1024);
        assert_eq!(
            checked_request_body_mutation_total(8 * 1024, 1, small_cap),
            Err(STATUS_INTERNAL_FAILURE),
            "a small physical cap must not inherit a fixed 64 MiB work allowance"
        );

        assert_eq!(request_body_mutation_work_limit(usize::MAX), usize::MAX);
        assert_eq!(
            checked_request_body_mutation_total(usize::MAX - 1, 1, usize::MAX),
            Ok(usize::MAX)
        );
        assert_eq!(
            checked_request_body_mutation_total(usize::MAX, 1, usize::MAX),
            Err(STATUS_INTERNAL_FAILURE),
            "cumulative work overflow must fail closed"
        );
    }

    /// The import remains defined for every buffer type. Request Body mutation
    /// is enabled only during the request-body phase; response Body mutation
    /// remains a truthful `UNIMPLEMENTED` result rather than trapping.
    const SET_BUFFER_BYTES_GUEST: &str = r#"
    (module
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "XYZ")
      (func (export "proxy_on_context_create") (param i32 i32))
      (func (export "proxy_on_vm_start") (param i32 i32) (result i32) (i32.const 1))
      (func (export "proxy_on_configure") (param i32 i32) (result i32) (i32.const 1))
      (func (export "call") (param i32 i32 i32 i32 i32) (result i32)
        (call $set (local.get 0) (local.get 1) (local.get 2)
                   (local.get 3) (local.get 4)))
    )
    "#;

    #[test]
    fn set_buffer_bytes_enforces_phase_and_buffer_type() {
        let (mut store, instance) = instantiate_guest(SET_BUFFER_BYTES_GUEST);
        let f = instance
            .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "call")
            .unwrap();

        assert_eq!(
            f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 0, 0, 0, 3)).unwrap(),
            STATUS_BAD_ARGUMENT
        );
        store
            .data_mut()
            .begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
        store.data_mut().request_body = b"abcdef".to_vec();
        store.data_mut().request_body_limit = 16;
        assert_eq!(
            f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 1, 2, 0, 3)).unwrap(),
            STATUS_OK
        );
        assert_eq!(store.data().request_body, b"aXYZdef");
        assert!(store.data().request_body_modified);
        assert_eq!(
            f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 99, 0, i32::MAX, 3)).unwrap(),
            STATUS_BAD_ARGUMENT,
            "an impossible range must be rejected before guest-memory copying"
        );
        assert_eq!(
            f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 0, 0, i32::MAX, 3)).unwrap(),
            STATUS_INVALID_MEMORY_ACCESS
        );
        let mutation_limit = request_body_mutation_work_limit(store.data().request_body_limit);
        store.data_mut().request_body_mutation_work = mutation_limit;
        let body_before = store.data().request_body.clone();
        assert_eq!(
            f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 0, 3, i32::MAX, 3)).unwrap(),
            STATUS_INTERNAL_FAILURE,
            "an exhausted work budget must reject before guest-memory copying"
        );
        assert_eq!(store.data().request_body, body_before);
        assert_eq!(
            f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 0, 0, 0, -1)).unwrap(),
            STATUS_BAD_ARGUMENT
        );
        assert_eq!(
            f.call(&mut store, (BUF_HTTP_RESPONSE_BODY, 0, 0, 0, 0)).unwrap(),
            STATUS_UNIMPLEMENTED
        );
    }
}
