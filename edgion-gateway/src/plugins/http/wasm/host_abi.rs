//! proxy-wasm ABI v0.2 host bridge (headers + local response subset).
//!
//! Design: host functions never touch the PluginSession. The plugin snapshots
//! request/response headers into `HostState` before invoking the guest, the
//! guest mutates the snapshot (so it can read back its own writes) while ops
//! are also queued, and the plugin drains `pending_ops` onto the session after
//! the guest returns. Buffers returned to the guest are allocated by calling
//! the guest's own `proxy_on_memory_allocate` export.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::ops::Range;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use wasmtime::{Caller, Extern, FuncType, Linker, ValType};

use edgion_wasm::{host, HostBehavior, WasmValueType};

use super::shared_data::{SharedData, SharedDataError, ENTRY_OVERHEAD_BYTES, MAX_SHARED_VALUE_BYTES};

pub const MAP_HTTP_REQUEST_HEADERS: i32 = 0;
pub const MAP_HTTP_RESPONSE_HEADERS: i32 = 2;
pub const MAP_HTTP_RESPONSE_TRAILERS: i32 = 3;
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

pub use super::limits::{
    MAX_GUEST_MEMORY_BYTES, MAX_GUEST_TABLE_ELEMENTS, MAX_RETAINED_ABI_BYTES, MAX_RETAINED_ABI_OPS,
};

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
/// Conservative hash-table storage used by preflight profiles. The standard
/// tables keep spare buckets and control bytes; four value slots per inserted
/// item also covers their small-table minimum without depending on internals.
const HASH_TABLE_SLOTS_PER_ITEM: usize = 4;
const HASH_TABLE_CONTROL_BYTES: usize = 16;

/// Per-host-call allocation ceiling. The guest-memory limiter already bounds
/// one input range to 128 MiB; applying that same envelope to the complete
/// concurrently-live host representation prevents decoding from multiplying
/// one valid range into a much larger allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AbiAllocationLimits {
    transient_bytes: usize,
    retained_bytes: usize,
    retained_ops: usize,
}

impl Default for AbiAllocationLimits {
    fn default() -> Self {
        Self {
            transient_bytes: super::admission::limits().max_host_call_bytes,
            retained_bytes: super::admission::limits().max_host_abi_bytes,
            retained_ops: super::admission::limits().max_host_abi_ops,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AllocationProfile {
    bytes: usize,
}

impl AllocationProfile {
    fn add_bytes(mut self, bytes: usize) -> Result<Self, i32> {
        self.bytes = self.bytes.checked_add(bytes).ok_or(STATUS_INTERNAL_FAILURE)?;
        Ok(self)
    }

    fn add_items<T>(self, count: usize) -> Result<Self, i32> {
        self.add_bytes(count.checked_mul(size_of::<T>()).ok_or(STATUS_INTERNAL_FAILURE)?)
    }

    fn repeated(self, count: usize) -> Result<Self, i32> {
        Ok(Self {
            bytes: self.bytes.checked_mul(count).ok_or(STATUS_INTERNAL_FAILURE)?,
        })
    }

    fn checked_against(self, limit: usize, status: i32) -> Result<Self, i32> {
        if self.bytes > limit {
            return Err(status);
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WasmPhase {
    #[default]
    RequestHeaders,
    ResponseHeaders,
    ResponseBody,
    ResponseTrailers,
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

/// Per-Store persistent configuration and execution identity. Callback data is
/// installed only for the currently executing event and released afterwards.
#[derive(Default)]
pub struct HostState {
    pub(super) guest_memory: Arc<super::admission::GuestMemory>,
    pending_memory_growth: usize,
    pending_table_growth: usize,
    pub frame: CallbackFrame,
    pub vm_config: Vec<u8>,
    pub plugin_config: Bytes,
    /// Metrics label of the owning module (set once per Store).
    pub metrics_label: String,
    last_callout_token: u32,

    /// Guest-requested tick period in ms (0 = no tick). Module-lifetime
    /// state (typically set during `on_configure`), NOT reset per phase;
    /// the module tick task reads it after instance init.
    pub tick_period_ms: u64,

    /// Module-wide shared KV (proxy-wasm shared data). Shared by every
    /// pooled VM of the same `ModuleEntry`; process-local by design.
    pub shared_data: Arc<SharedData>,
    /// Startup limits, never larger than the reviewed safety ceilings.
    /// Unit tests may lower them further to exercise boundaries cheaply.
    allocation_limits: AbiAllocationLimits,
}

/// Bounded temporary host payloads for one guest callback sequence.
#[derive(Default)]
pub struct CallbackFrame {
    pub read_only: bool,
    pub response_trailers: Vec<(String, String)>,

    pub phase: WasmPhase,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub pending_ops: Vec<HeaderOp>,
    pub local_response: Option<LocalResponse>,
    pub properties: HashMap<String, String>,
    /// (level, message) lines emitted via proxy_log during the current phase.
    pub log_lines: Vec<(i32, String)>,

    // ---- Callout state (armed per phase; see `arm_dispatch`) ----
    /// Whether `proxy_dispatch_http_call` is accepted in the current phase.
    pub dispatch_enabled: bool,
    /// Normalized origins the guest may call out to (empty = deny all).
    pub callout_allowlist: Option<Arc<[String]>>,
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

    // ---- Body state ----
    /// Current response-body chunk exposed via
    /// `get_buffer_bytes(HttpResponseBody)`. Never buffers across chunks.
    pub body_chunk: Bytes,

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
    /// Release completed callback payloads before retaining an idle instance.
    /// This is host-only cleanup: guest memory, configuration, shared data and
    /// the root timer survive. Replacing containers releases their capacity.
    pub fn release_phase_payloads(&mut self) {
        self.frame = CallbackFrame::default();
    }

    pub fn new(
        vm_config: Vec<u8>,
        plugin_config: Vec<u8>,
        metrics_label: String,
        shared_data: Arc<SharedData>,
    ) -> Self {
        HostState {
            vm_config,
            plugin_config: plugin_config.into(),
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
        self.frame = CallbackFrame {
            phase,
            request_headers,
            response_headers,
            properties,
            ..CallbackFrame::default()
        };
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
        self.checked_abi_charge_with_limits(
            payload,
            self.allocation_limits.retained_bytes,
            self.allocation_limits.retained_ops,
        )
    }

    fn checked_abi_charge_with_limits(
        &self,
        payload: usize,
        max_bytes: usize,
        max_ops: usize,
    ) -> Result<(usize, usize), i32> {
        let cost = payload.saturating_add(ABI_OP_OVERHEAD_BYTES);
        let new_bytes = self.frame.retained_bytes.saturating_add(cost);
        let new_ops = self.frame.retained_ops.saturating_add(1);
        if new_bytes > max_bytes || new_ops > max_ops {
            return Err(STATUS_INTERNAL_FAILURE);
        }
        Ok((new_bytes, new_ops))
    }

    fn commit_abi_charge(&mut self, checked: (usize, usize)) {
        self.frame.retained_bytes = checked.0;
        self.frame.retained_ops = checked.1;
    }

    fn check_transient_allocation(&self, profile: AllocationProfile) -> Result<(), i32> {
        profile
            .checked_against(self.allocation_limits.transient_bytes, STATUS_INTERNAL_FAILURE)
            .map(drop)
    }

    /// Validate a request-body mutation charge without changing counters.
    ///
    /// The ABI bridge uses this before copying guest memory so a guest cannot
    /// keep forcing large transient reads after exhausting the work budget.
    fn check_request_body_mutation_charge(&self, work: usize) -> Result<(), i32> {
        self.checked_abi_charge(0)?;
        checked_request_body_mutation_total(
            self.frame.request_body_mutation_work,
            work,
            self.frame.request_body_limit,
        )
        .map(drop)
    }

    /// Commit a previously validated request-body mutation charge.
    fn charge_request_body_mutation(&mut self, work: usize) -> Result<(), i32> {
        let new_work = checked_request_body_mutation_total(
            self.frame.request_body_mutation_work,
            work,
            self.frame.request_body_limit,
        )?;
        self.charge_abi(0)?;
        self.frame.request_body_mutation_work = new_work;
        Ok(())
    }

    /// Enable `proxy_dispatch_http_call` for the current phase (request and
    /// tick phases — the sync response stages cannot await a callout).
    pub fn arm_dispatch(&mut self, allowlist: Arc<[String]>, timeout_cap_ms: u64) {
        self.frame.dispatch_enabled = true;
        self.frame.callout_allowlist = Some(allowlist);
        self.frame.callout_timeout_cap_ms = timeout_cap_ms;
    }

    fn map_ref(&self, map: i32) -> Option<&[(String, String)]> {
        match map {
            MAP_HTTP_REQUEST_HEADERS => Some(&self.frame.request_headers),
            MAP_HTTP_RESPONSE_HEADERS => Some(&self.frame.response_headers),
            MAP_HTTP_RESPONSE_TRAILERS => Some(&self.frame.response_trailers),
            // The callout-response map is only populated during a
            // `proxy_on_http_call_response` delivery; elsewhere it reads empty
            // (not an error). Returned as a borrow so a per-header lookup does
            // not clone the whole map.
            MAP_HTTP_CALL_RESPONSE_HEADERS => Some(
                self.frame
                    .callout_response
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
    fn memory_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        self.pending_memory_growth = 0;
        if desired > super::admission::limits().max_instance_memory_bytes || maximum.is_some_and(|max| desired > max) {
            return Ok(false);
        }
        let Some(delta) = desired.checked_sub(current) else {
            return Ok(false);
        };
        if !self.guest_memory.reserve(delta) {
            return Ok(false);
        }
        self.pending_memory_growth = delta;
        Ok(true)
    }

    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        self.guest_memory
            .rollback(std::mem::take(&mut self.pending_memory_growth));
        Ok(())
    }

    fn table_growing(&mut self, current: usize, desired: usize, maximum: Option<usize>) -> wasmtime::Result<bool> {
        self.pending_table_growth = 0;
        if desired > super::admission::limits().max_instance_table_elements || maximum.is_some_and(|max| desired > max)
        {
            return Ok(false);
        }
        // Wasmtime 49 stores one pointer-sized slot per table element.
        let Some(delta) = desired
            .checked_sub(current)
            .and_then(|n| n.checked_mul(size_of::<usize>()))
        else {
            return Ok(false);
        };
        if !self.guest_memory.reserve(delta) {
            return Ok(false);
        }
        self.pending_table_growth = delta;
        Ok(true)
    }

    fn table_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        self.guest_memory
            .rollback(std::mem::take(&mut self.pending_table_growth));
        Ok(())
    }

    // Match the Engine pooling profile explicitly; these are per Store.
    fn instances(&self) -> usize {
        1
    }
    fn memories(&self) -> usize {
        1
    }
    fn tables(&self) -> usize {
        1
    }
}

// ---------- proxy-wasm serialized map codec ----------

pub fn encode_map(pairs: &[(String, String)]) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(size_of::<u32>() + pairs.iter().map(|(k, v)| k.len() + v.len() + 10).sum::<usize>());
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

fn try_vec_with_capacity<T>(capacity: usize) -> Result<Vec<T>, i32> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| STATUS_INTERNAL_FAILURE)?;
    Ok(values)
}

fn try_string_with_capacity(capacity: usize) -> Result<String, i32> {
    let mut value = String::new();
    value.try_reserve_exact(capacity).map_err(|_| STATUS_INTERNAL_FAILURE)?;
    Ok(value)
}

fn push_utf8_lossy(output: &mut String, mut remaining: &[u8]) {
    loop {
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                output.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                // `valid_up_to` always names a valid UTF-8 prefix.
                if let Ok(valid_prefix) = std::str::from_utf8(&remaining[..valid]) {
                    output.push_str(valid_prefix);
                }
                output.push('\u{FFFD}');
                let invalid = error.error_len().unwrap_or(remaining.len() - valid);
                remaining = &remaining[valid + invalid..];
            }
        }
    }
}

fn string_from_utf8_lossy_with_capacity(bytes: &[u8], capacity: usize) -> Result<String, i32> {
    let mut output = try_string_with_capacity(capacity)?;
    push_utf8_lossy(&mut output, bytes);
    debug_assert_eq!(output.len(), capacity);
    Ok(output)
}

fn try_clone_string(value: &str) -> Result<String, i32> {
    let mut clone = try_string_with_capacity(value.len())?;
    clone.push_str(value);
    Ok(clone)
}

fn try_ascii_lowercase(value: &str) -> Result<String, i32> {
    let mut normalized = try_string_with_capacity(value.len())?;
    normalized.push_str(value);
    normalized.make_ascii_lowercase();
    debug_assert_eq!(normalized.len(), value.len());
    Ok(normalized)
}

fn conservative_hash_table_allocation<T>(items: usize) -> Result<AllocationProfile, i32> {
    if items == 0 {
        return Ok(AllocationProfile::default());
    }
    AllocationProfile::default()
        .add_items::<T>(
            items
                .checked_mul(HASH_TABLE_SLOTS_PER_ITEM)
                .ok_or(STATUS_INTERNAL_FAILURE)?,
        )?
        .add_bytes(HASH_TABLE_CONTROL_BYTES)
}

fn vector_growth_allocation<T>(len: usize, capacity: usize, additional: usize) -> Result<AllocationProfile, i32> {
    let required = len.checked_add(additional).ok_or(STATUS_INTERNAL_FAILURE)?;
    if required <= capacity {
        return Ok(AllocationProfile::default());
    }
    AllocationProfile::default().add_items::<T>(required)
}

fn decoded_map_allocation(profile: SerializedMapProfile) -> Result<AllocationProfile, i32> {
    AllocationProfile::default()
        .add_bytes(profile.lossy_string_bytes)?
        .add_items::<(String, String)>(profile.pair_count)
}

fn local_response_allocation(response: &LocalResponse) -> Result<AllocationProfile, i32> {
    let mut profile = AllocationProfile::default()
        .add_bytes(response.body.capacity())?
        .add_items::<(String, String)>(response.headers.capacity())?;
    for (name, value) in &response.headers {
        profile = profile.add_bytes(name.capacity())?.add_bytes(value.capacity())?;
    }
    Ok(profile)
}

fn decode_profiled_map(bytes: &[u8], profile: SerializedMapProfile) -> Result<Vec<(String, String)>, i32> {
    let table_end = 4 + profile.pair_count * 8;
    let mut data_offset = table_end;
    let mut pairs = try_vec_with_capacity(profile.pair_count)?;
    for index in 0..profile.pair_count {
        let size_offset = 4 + index * 8;
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
        let key_end = data_offset + key_len;
        let value_start = key_end + 1;
        let value_end = value_start + value_len;
        let key_bytes = &bytes[data_offset..key_end];
        let value_bytes = &bytes[value_start..value_end];
        let key_capacity = lossy_utf8_output_len(key_bytes)?;
        let value_capacity = lossy_utf8_output_len(value_bytes)?;
        pairs.push((
            string_from_utf8_lossy_with_capacity(key_bytes, key_capacity)?,
            string_from_utf8_lossy_with_capacity(value_bytes, value_capacity)?,
        ));
        data_offset = value_end + 1;
    }
    Ok(pairs)
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
    decode_profiled_map(bytes, profile).ok()
}

fn simple_header_mutation_profile(
    key_bytes: usize,
    value_bytes: usize,
    snapshot_copy: bool,
) -> Result<AllocationProfile, i32> {
    let one_copy = AllocationProfile::default()
        .add_bytes(key_bytes)?
        .add_bytes(value_bytes)?;
    let mut profile = one_copy;
    if snapshot_copy {
        profile = profile.add_bytes(one_copy.bytes)?.add_items::<(String, String)>(1)?;
    }
    Ok(profile)
}

fn try_clone_pairs(pairs: &[(String, String)]) -> Result<Vec<(String, String)>, i32> {
    let mut cloned = try_vec_with_capacity(pairs.len())?;
    for (name, value) in pairs {
        cloned.push((try_clone_string(name)?, try_clone_string(value)?));
    }
    Ok(cloned)
}

fn header_removal_profiles(pairs: &[(String, String)]) -> Result<(AllocationProfile, AllocationProfile), i32> {
    let name_bytes = pairs.iter().try_fold(0usize, |total, (name, _)| {
        total.checked_add(name.len()).ok_or(STATUS_INTERNAL_FAILURE)
    })?;
    // `remove` retains at most one original-spelling String per input pair.
    let retained = AllocationProfile::default()
        .add_bytes(name_bytes)?
        .add_items::<String>(pairs.len())?;
    // Include both canonical name allocations and a conservative upper bound
    // for the HashSet's spare buckets/control bytes.
    let transient = retained
        .add_bytes(name_bytes)?
        .add_bytes(conservative_hash_table_allocation::<String>(pairs.len())?.bytes)?;
    Ok((retained, transient))
}

fn collect_header_removals(pairs: &[(String, String)]) -> Result<Vec<String>, i32> {
    let mut seen = HashSet::new();
    seen.try_reserve(pairs.len()).map_err(|_| STATUS_INTERNAL_FAILURE)?;
    let mut remove = try_vec_with_capacity(pairs.len())?;
    for (name, _) in pairs {
        let canonical = try_ascii_lowercase(name)?;
        if seen.insert(canonical) {
            remove.push(try_clone_string(name)?);
        }
    }
    Ok(remove)
}

/// Borrow a recognized ABI source without copying its payload.
/// HostState remains the source owner; only the selected range crosses guest reentry.
enum BufferSource<'a> {
    Borrowed(&'a [u8]),
    Shared(&'a Bytes),
}

enum BufferSnapshot {
    Owned(Vec<u8>),
    Shared(Bytes),
}

impl AsRef<[u8]> for BufferSnapshot {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

impl BufferSource<'_> {
    /// Own the validated range before guest allocation can reenter and mutate
    /// HostState. The returned snapshot has no borrow of the source owner.
    fn snapshot(self, start: usize, max_size: usize) -> Result<BufferSnapshot, i32> {
        let len = match &self {
            Self::Borrowed(bytes) => bytes.len(),
            Self::Shared(bytes) => bytes.len(),
        };
        if start >= len && len != 0 {
            return Err(STATUS_BAD_ARGUMENT);
        }
        let range = buffer_range(len, start, max_size);
        match self {
            Self::Borrowed(bytes) => {
                let mut snapshot = try_vec_with_capacity(range.len())?;
                snapshot.extend_from_slice(&bytes[range]);
                Ok(BufferSnapshot::Owned(snapshot))
            }
            Self::Shared(bytes) => Ok(BufferSnapshot::Shared(bytes.slice(range))),
        }
    }
}

/// `None` means an unknown buffer type; absent callout data is recognized empty.
fn select_buffer(state: &HostState, buf_type: i32) -> Option<BufferSource<'_>> {
    match buf_type {
        BUF_VM_CONFIGURATION => Some(BufferSource::Borrowed(&state.vm_config)),
        BUF_PLUGIN_CONFIGURATION => Some(BufferSource::Borrowed(&state.plugin_config)),
        BUF_HTTP_CALL_RESPONSE_BODY => Some(BufferSource::Borrowed(
            state
                .frame
                .callout_response
                .as_ref()
                .map(|r| r.body.as_slice())
                .unwrap_or_default(),
        )),
        // Buffer mode: the full request body staged by the body gate.
        BUF_HTTP_REQUEST_BODY => Some(BufferSource::Borrowed(&state.frame.request_body)),
        // Read-only chunk window: the CURRENT chunk, never an accumulated body.
        BUF_HTTP_RESPONSE_BODY => Some(BufferSource::Shared(&state.frame.body_chunk)),
        _ => None,
    }
}

/// Clamp the range after validating the ABI's nonempty-source offset contract.
/// Empty sources accept arbitrary offsets and resolve to an empty range.
fn buffer_range(len: usize, start: usize, max_size: usize) -> Range<usize> {
    let start = start.min(len);
    let end = len.min(start.saturating_add(max_size));
    start..end
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
    if state.frame.phase != WasmPhase::RequestBody {
        return Err(STATUS_BAD_ARGUMENT);
    }
    let end = start.checked_add(size).ok_or(STATUS_BAD_ARGUMENT)?;
    if start > state.frame.request_body.len() || end > state.frame.request_body.len() {
        return Err(STATUS_BAD_ARGUMENT);
    }
    let new_len = state
        .frame
        .request_body
        .len()
        .checked_sub(size)
        .and_then(|len| len.checked_add(replacement_len))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    if new_len > state.frame.request_body_limit {
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
    let work = request_body_mutation_work(state.frame.request_body.len(), &range, replacement.len());
    if let Err(status) = state.charge_request_body_mutation(work) {
        return status;
    }
    if range.is_empty() && replacement.is_empty() {
        return STATUS_OK;
    }
    state.frame.request_body.splice(range, replacement);
    state.frame.request_body_modified = true;
    STATUS_OK
}

// ---------- guest memory helpers ----------

fn guest_memory(caller: &mut Caller<'_, HostState>) -> Option<wasmtime::Memory> {
    match caller.get_export("memory") {
        Some(Extern::Memory(m)) => Some(m),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GuestRangeProfile {
    range: Range<usize>,
}

impl GuestRangeProfile {
    fn len(&self) -> usize {
        self.range.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GuestStringProfile {
    guest: GuestRangeProfile,
    decoded_len: usize,
}

#[derive(Clone)]
struct GuestMapProfile {
    guest: GuestRangeProfile,
    map: SerializedMapProfile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PropertyPathProfile {
    guest: GuestRangeProfile,
    decoded_len: usize,
    segment_count: usize,
}

fn profile_guest_memory_range(
    caller: &mut Caller<'_, HostState>,
    ptr: i32,
    len: i32,
) -> Result<(wasmtime::Memory, GuestRangeProfile), i32> {
    // Preserve ABI precedence: negative arguments are malformed even when the
    // guest failed to export memory.
    if ptr < 0 || len < 0 {
        return Err(STATUS_BAD_ARGUMENT);
    }
    let memory = guest_memory(caller).ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let range = guest_range(ptr, len, memory.data_size(&*caller))?;
    Ok((memory, GuestRangeProfile { range }))
}

fn profile_guest_range(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<GuestRangeProfile, i32> {
    profile_guest_memory_range(caller, ptr, len).map(|(_, guest)| guest)
}

fn profile_guest_string(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<GuestStringProfile, i32> {
    let (memory, guest) = profile_guest_memory_range(caller, ptr, len)?;
    let bytes = memory
        .data(&*caller)
        .get(guest.range.clone())
        .ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let decoded_len = lossy_utf8_output_len(bytes)?;
    Ok(GuestStringProfile { guest, decoded_len })
}

fn profile_guest_map(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<GuestMapProfile, i32> {
    let (memory, guest) = profile_guest_memory_range(caller, ptr, len)?;
    let bytes = memory
        .data(&*caller)
        .get(guest.range.clone())
        .ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let map = scan_serialized_map(bytes)?;
    Ok(GuestMapProfile { guest, map })
}

fn profile_property_path(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<PropertyPathProfile, i32> {
    let (memory, guest) = profile_guest_memory_range(caller, ptr, len)?;
    let bytes = memory
        .data(&*caller)
        .get(guest.range.clone())
        .ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let mut decoded_len = 0usize;
    let mut segment_count = 0usize;
    for segment in bytes.split(|byte| *byte == 0).filter(|segment| !segment.is_empty()) {
        if segment_count != 0 {
            decoded_len = decoded_len.checked_add(1).ok_or(STATUS_INTERNAL_FAILURE)?;
        }
        decoded_len = decoded_len
            .checked_add(lossy_utf8_output_len(segment)?)
            .ok_or(STATUS_INTERNAL_FAILURE)?;
        segment_count = segment_count.checked_add(1).ok_or(STATUS_INTERNAL_FAILURE)?;
    }
    Ok(PropertyPathProfile {
        guest,
        decoded_len,
        segment_count,
    })
}

fn guest_bytes<'a>(
    caller: &'a Caller<'_, HostState>,
    memory: wasmtime::Memory,
    profile: &GuestRangeProfile,
) -> Result<&'a [u8], i32> {
    memory
        .data(caller)
        .get(profile.range.clone())
        .ok_or(STATUS_INVALID_MEMORY_ACCESS)
}

fn copy_profiled_guest(caller: &mut Caller<'_, HostState>, profile: &GuestRangeProfile) -> Result<Vec<u8>, i32> {
    let memory = guest_memory(caller).ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let bytes = guest_bytes(&*caller, memory, profile)?;
    let mut output = try_vec_with_capacity(bytes.len())?;
    output.extend_from_slice(bytes);
    Ok(output)
}

fn decode_profiled_guest_string(
    caller: &mut Caller<'_, HostState>,
    profile: &GuestStringProfile,
) -> Result<String, i32> {
    let memory = guest_memory(caller).ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let bytes = guest_bytes(&*caller, memory, &profile.guest)?;
    string_from_utf8_lossy_with_capacity(bytes, profile.decoded_len)
}

fn decode_profiled_guest_map(
    caller: &mut Caller<'_, HostState>,
    profile: &GuestMapProfile,
) -> Result<Vec<(String, String)>, i32> {
    let memory = guest_memory(caller).ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let bytes = guest_bytes(&*caller, memory, &profile.guest)?;
    decode_profiled_map(bytes, profile.map)
}

fn decode_property_path(caller: &mut Caller<'_, HostState>, profile: &PropertyPathProfile) -> Result<String, i32> {
    let memory = guest_memory(caller).ok_or(STATUS_INVALID_MEMORY_ACCESS)?;
    let bytes = guest_bytes(&*caller, memory, &profile.guest)?;
    let mut key = try_string_with_capacity(profile.decoded_len)?;
    let mut first = true;
    let mut segment_count = 0usize;
    for segment in bytes.split(|byte| *byte == 0).filter(|segment| !segment.is_empty()) {
        if !first {
            key.push('.');
        }
        push_utf8_lossy(&mut key, segment);
        first = false;
        segment_count += 1;
    }
    debug_assert_eq!(key.len(), profile.decoded_len);
    debug_assert_eq!(segment_count, profile.segment_count);
    Ok(key)
}

fn read_guest(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<Vec<u8>, i32> {
    let profile = profile_guest_range(caller, ptr, len)?;
    copy_profiled_guest(caller, &profile)
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
    let profile = profile_guest_string(caller, ptr, len)?;
    caller
        .data()
        .check_transient_allocation(AllocationProfile::default().add_bytes(profile.decoded_len)?)?;
    decode_profiled_guest_string(caller, &profile)
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

struct CalloutAllocationProfile {
    payload_bytes: usize,
    headers: SerializedMapProfile,
}

fn profile_callout_allocation(
    upstream_bytes: &[u8],
    header_bytes: &[u8],
    body_size: usize,
) -> Result<CalloutAllocationProfile, i32> {
    let profile = scan_serialized_map(header_bytes)?;
    // Retain the conservative raw-header allowance even though decoding now
    // borrows those bytes, preserving the existing admission boundary.
    let raw_guest_copies = upstream_bytes
        .len()
        .checked_add(header_bytes.len())
        .and_then(|len| len.checked_add(body_size))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    let upstream_lossy_bytes = lossy_utf8_output_len(upstream_bytes)?;
    let lossy_owned_bytes = upstream_lossy_bytes
        .checked_add(profile.lossy_string_bytes)
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    // The decoded map's Vec is consumed while the filtered PendingCallout header
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
    let payload_bytes = raw_guest_copies
        .checked_add(lossy_owned_bytes)
        .and_then(|bytes| bytes.checked_add(pair_vec_bytes))
        .and_then(|bytes| bytes.checked_add(origin_bytes))
        .and_then(|bytes| bytes.checked_add(callout_fixed_bytes))
        .ok_or(STATUS_INTERNAL_FAILURE)?;
    Ok(CalloutAllocationProfile {
        payload_bytes,
        headers: profile,
    })
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
    if !caller.data().frame.dispatch_enabled {
        return STATUS_UNIMPLEMENTED;
    }
    if caller.data().frame.callouts_dispatched >= MAX_CALLOUTS_PER_REQUEST {
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
        match profile_callout_allocation(&memory_bytes[upstream_range], &memory_bytes[headers_range], body_len) {
            Ok(payload) => payload,
            Err(status) => return status,
        }
    };
    if let Err(status) = caller.data().checked_abi_charge(allocation_preflight.payload_bytes) {
        return status;
    }
    let upstream = match read_guest_string(&mut caller, upstream_ptr, upstream_size) {
        Ok(s) => s,
        Err(s) => return s,
    };
    let all_headers = {
        let (memory, profile) = match profile_guest_memory_range(&mut caller, headers_ptr, headers_size) {
            Ok(profile) => profile,
            Err(status) => return status,
        };
        let bytes = match guest_bytes(&caller, memory, &profile) {
            Ok(bytes) => bytes,
            Err(status) => return status,
        };
        // No guest execution or memory mutation occurs after profiling. Decode
        // directly into owned strings before releasing this guest-memory borrow.
        // Preserve decode_map's BAD_ARGUMENT mapping for decoder failures.
        let Ok(headers) = decode_profiled_map(bytes, allocation_preflight.headers) else {
            return STATUS_BAD_ARGUMENT;
        };
        headers
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
    let Some(origin) = edgion_resources::plugin_configs::normalize_callout_origin_quiet(&upstream) else {
        // Data-plane hot path (Iron rule 3), guest-supplied origin (Iron rule 1):
        // reject via status code only, no tracing or raw-bearing error String.
        return STATUS_BAD_ARGUMENT;
    };

    let state = caller.data_mut();
    if !state
        .frame
        .callout_allowlist
        .as_ref()
        .is_some_and(|allowlist| allowlist.iter().any(|a| a == &origin))
    {
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

    let Some(token) = state.last_callout_token.checked_add(1) else {
        return STATUS_INTERNAL_FAILURE;
    };
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
    state.frame.callouts_dispatched += 1;
    state.last_callout_token = token;
    state.frame.pending_callouts.push(pending);
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
            if caller.data().frame.read_only {
                return STATUS_UNIMPLEMENTED;
            }
            let profile = match profile_guest_string(&mut caller, ptr, len) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let allocation = match AllocationProfile::default().add_bytes(profile.decoded_len) {
                Ok(profile) => match vector_growth_allocation::<(i32, String)>(
                    caller.data().frame.log_lines.len(),
                    caller.data().frame.log_lines.capacity(),
                    1,
                )
                .and_then(|growth| profile.add_bytes(growth.bytes))
                {
                    Ok(profile) => profile,
                    Err(status) => return status,
                },
                Err(status) => return status,
            };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            let checked_charge = match caller.data().checked_abi_charge(allocation.bytes) {
                Ok(charge) => charge,
                Err(status) => return status,
            };
            let msg = match decode_profiled_guest_string(&mut caller, &profile) {
                Ok(msg) => msg,
                Err(status) => return status,
            };
            let state = caller.data_mut();
            if state.frame.log_lines.try_reserve_exact(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            // Data-plane hot path (Iron rule 3) and a guest-controlled
            // message (Iron rule 1): never tracing the raw text. The line is
            // retained only for bounded diagnostics and reported as a count.
            state.commit_abi_charge(checked_charge);
            state.frame.log_lines.push((level, msg));
            STATUS_OK
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
            let key_profile = match profile_guest_string(&mut caller, kp, kl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let key_allocation = match AllocationProfile::default().add_bytes(key_profile.decoded_len) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            if let Err(status) = caller.data().check_transient_allocation(key_allocation) {
                return status;
            }
            let key = match decode_profiled_guest_string(&mut caller, &key_profile) {
                Ok(key) => key,
                Err(status) => return status,
            };
            let value = {
                let Some(pairs) = caller.data().map_ref(map) else {
                    return STATUS_BAD_ARGUMENT;
                };
                let Some((_, value)) = pairs.iter().find(|(name, _)| name.eq_ignore_ascii_case(&key)) else {
                    return STATUS_NOT_FOUND;
                };
                let allocation = match AllocationProfile::default()
                    .add_bytes(key.len())
                    .and_then(|profile| profile.add_bytes(value.len()))
                {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
                if let Err(status) = caller.data().check_transient_allocation(allocation) {
                    return status;
                }
                match try_clone_string(value) {
                    Ok(value) => value,
                    Err(status) => return status,
                }
            };
            // The owned value survives allocator reentry; the lookup key is done.
            drop(key);
            write_to_guest(&mut caller, value.as_bytes(), ret_ptr, ret_size)
        },
    )?;

    linker.func_wrap(
        host::MODULE,
        host::PROXY_REPLACE_HEADER_MAP_VALUE,
        |mut caller: Caller<'_, HostState>, map: i32, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
            if caller.data().frame.read_only {
                return STATUS_UNIMPLEMENTED;
            }
            let key_profile = match profile_guest_string(&mut caller, kp, kl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let value_profile = match profile_guest_string(&mut caller, vp, vl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let allocation =
                match simple_header_mutation_profile(key_profile.decoded_len, value_profile.decoded_len, true) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            if let Err(status) = caller.data().checked_abi_charge(allocation.bytes) {
                return status;
            }
            if !matches!(map, MAP_HTTP_REQUEST_HEADERS | MAP_HTTP_RESPONSE_HEADERS) {
                return STATUS_BAD_ARGUMENT;
            }
            let allocation = {
                let state = caller.data();
                let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                    &state.frame.request_headers
                } else {
                    &state.frame.response_headers
                };
                let snapshot_growth =
                    match vector_growth_allocation::<(String, String)>(pairs.len(), pairs.capacity(), 1) {
                        Ok(profile) => profile,
                        Err(status) => return status,
                    };
                let pending_growth = match vector_growth_allocation::<HeaderOp>(
                    state.frame.pending_ops.len(),
                    state.frame.pending_ops.capacity(),
                    1,
                ) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
                match allocation
                    .add_bytes(snapshot_growth.bytes)
                    .and_then(|profile| profile.add_bytes(pending_growth.bytes))
                {
                    Ok(profile) => profile,
                    Err(status) => return status,
                }
            };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            let checked_charge = match caller.data().checked_abi_charge(allocation.bytes) {
                Ok(charge) => charge,
                Err(status) => return status,
            };
            let key = match decode_profiled_guest_string(&mut caller, &key_profile) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let value = match decode_profiled_guest_string(&mut caller, &value_profile) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let snapshot_key = match try_clone_string(&key) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let snapshot_value = match try_clone_string(&value) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let state = caller.data_mut();
            if state.frame.pending_ops.try_reserve_exact(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                &mut state.frame.request_headers
            } else {
                &mut state.frame.response_headers
            };
            if pairs.try_reserve_exact(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            state.commit_abi_charge(checked_charge);
            let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                &mut state.frame.request_headers
            } else {
                &mut state.frame.response_headers
            };
            pairs.retain(|(name, _)| !name.eq_ignore_ascii_case(&key));
            pairs.push((snapshot_key, snapshot_value));
            state.frame.pending_ops.push(HeaderOp::Set { map, name: key, value });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        host::MODULE,
        host::PROXY_ADD_HEADER_MAP_VALUE,
        |mut caller: Caller<'_, HostState>, map: i32, kp: i32, kl: i32, vp: i32, vl: i32| -> i32 {
            if caller.data().frame.read_only {
                return STATUS_UNIMPLEMENTED;
            }
            let key_profile = match profile_guest_string(&mut caller, kp, kl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let value_profile = match profile_guest_string(&mut caller, vp, vl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let allocation =
                match simple_header_mutation_profile(key_profile.decoded_len, value_profile.decoded_len, true) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            if let Err(status) = caller.data().checked_abi_charge(allocation.bytes) {
                return status;
            }
            if !matches!(map, MAP_HTTP_REQUEST_HEADERS | MAP_HTTP_RESPONSE_HEADERS) {
                return STATUS_BAD_ARGUMENT;
            }
            let allocation = {
                let state = caller.data();
                let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                    &state.frame.request_headers
                } else {
                    &state.frame.response_headers
                };
                let snapshot_growth =
                    match vector_growth_allocation::<(String, String)>(pairs.len(), pairs.capacity(), 1) {
                        Ok(profile) => profile,
                        Err(status) => return status,
                    };
                let pending_growth = match vector_growth_allocation::<HeaderOp>(
                    state.frame.pending_ops.len(),
                    state.frame.pending_ops.capacity(),
                    1,
                ) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
                match allocation
                    .add_bytes(snapshot_growth.bytes)
                    .and_then(|profile| profile.add_bytes(pending_growth.bytes))
                {
                    Ok(profile) => profile,
                    Err(status) => return status,
                }
            };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            let checked_charge = match caller.data().checked_abi_charge(allocation.bytes) {
                Ok(charge) => charge,
                Err(status) => return status,
            };
            let key = match decode_profiled_guest_string(&mut caller, &key_profile) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let value = match decode_profiled_guest_string(&mut caller, &value_profile) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let snapshot_key = match try_clone_string(&key) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let snapshot_value = match try_clone_string(&value) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let state = caller.data_mut();
            if state.frame.pending_ops.try_reserve_exact(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                &mut state.frame.request_headers
            } else {
                &mut state.frame.response_headers
            };
            if pairs.try_reserve_exact(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            state.commit_abi_charge(checked_charge);
            let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                &mut state.frame.request_headers
            } else {
                &mut state.frame.response_headers
            };
            pairs.push((snapshot_key, snapshot_value));
            state.frame.pending_ops.push(HeaderOp::Add { map, name: key, value });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        host::MODULE,
        host::PROXY_REMOVE_HEADER_MAP_VALUE,
        |mut caller: Caller<'_, HostState>, map: i32, kp: i32, kl: i32| -> i32 {
            if caller.data().frame.read_only {
                return STATUS_UNIMPLEMENTED;
            }
            let key_profile = match profile_guest_string(&mut caller, kp, kl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let allocation = match simple_header_mutation_profile(key_profile.decoded_len, 0, false) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            if let Err(status) = caller.data().checked_abi_charge(allocation.bytes) {
                return status;
            }
            if !matches!(map, MAP_HTTP_REQUEST_HEADERS | MAP_HTTP_RESPONSE_HEADERS) {
                return STATUS_BAD_ARGUMENT;
            }
            let allocation = {
                let state = caller.data();
                let pending_growth = match vector_growth_allocation::<HeaderOp>(
                    state.frame.pending_ops.len(),
                    state.frame.pending_ops.capacity(),
                    1,
                ) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
                match allocation.add_bytes(pending_growth.bytes) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                }
            };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            let checked_charge = match caller.data().checked_abi_charge(allocation.bytes) {
                Ok(charge) => charge,
                Err(status) => return status,
            };
            let key = match decode_profiled_guest_string(&mut caller, &key_profile) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let state = caller.data_mut();
            if state.frame.pending_ops.try_reserve_exact(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            state.commit_abi_charge(checked_charge);
            let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                &mut state.frame.request_headers
            } else {
                &mut state.frame.response_headers
            };
            pairs.retain(|(name, _)| !name.eq_ignore_ascii_case(&key));
            state.frame.pending_ops.push(HeaderOp::Remove { map, name: key });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        host::MODULE,
        host::PROXY_SET_HEADER_MAP_PAIRS,
        |mut caller: Caller<'_, HostState>, map: i32, ptr: i32, len: i32| -> i32 {
            if caller.data().frame.read_only {
                return STATUS_UNIMPLEMENTED;
            }
            let map_profile = match profile_guest_map(&mut caller, ptr, len) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let decoded_allocation = match decoded_map_allocation(map_profile.map) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let base_allocation = match decoded_allocation.repeated(2) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            if let Err(status) = caller.data().check_transient_allocation(base_allocation) {
                return status;
            }
            if let Err(status) = caller.data().checked_abi_charge(base_allocation.bytes) {
                return status;
            }
            if !matches!(map, MAP_HTTP_REQUEST_HEADERS | MAP_HTTP_RESPONSE_HEADERS) {
                return STATUS_BAD_ARGUMENT;
            }

            let (removal_retained, removal_transient) = {
                let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                    &caller.data().frame.request_headers
                } else {
                    &caller.data().frame.response_headers
                };
                match header_removal_profiles(pairs) {
                    Ok(profiles) => profiles,
                    Err(status) => return status,
                }
            };
            let pending_growth = match vector_growth_allocation::<HeaderOp>(
                caller.data().frame.pending_ops.len(),
                caller.data().frame.pending_ops.capacity(),
                1,
            ) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let retained_allocation = match base_allocation
                .add_bytes(removal_retained.bytes)
                .and_then(|profile| profile.add_bytes(pending_growth.bytes))
            {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let transient_allocation = match base_allocation
                .add_bytes(removal_transient.bytes)
                .and_then(|profile| profile.add_bytes(pending_growth.bytes))
            {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            if let Err(status) = caller.data().check_transient_allocation(transient_allocation) {
                return status;
            }
            let checked_charge = match caller.data().checked_abi_charge(retained_allocation.bytes) {
                Ok(charge) => charge,
                Err(status) => return status,
            };
            let new_pairs = match decode_profiled_guest_map(&mut caller, &map_profile) {
                Ok(pairs) => pairs,
                Err(status) => return status,
            };
            let snapshot = match try_clone_pairs(&new_pairs) {
                Ok(pairs) => pairs,
                Err(status) => return status,
            };
            let remove = {
                let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                    &caller.data().frame.request_headers
                } else {
                    &caller.data().frame.response_headers
                };
                match collect_header_removals(pairs) {
                    Ok(remove) => remove,
                    Err(status) => return status,
                }
            };
            let state = caller.data_mut();
            if state.frame.pending_ops.try_reserve_exact(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            // Capture the names present before the replacement (case-insensitive
            // dedup) in expected-linear time. `remove` keeps the first observed
            // spelling while `seen` (discarded above) owns canonical keys.
            state.commit_abi_charge(checked_charge);
            let pairs = if map == MAP_HTTP_REQUEST_HEADERS {
                &mut state.frame.request_headers
            } else {
                &mut state.frame.response_headers
            };
            *pairs = snapshot;
            state.frame.pending_ops.push(HeaderOp::ReplaceAll {
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
            let snapshot = match data.snapshot(start as u32 as usize, max_size as u32 as usize) {
                Ok(snapshot) => snapshot,
                Err(status) => return status,
            };
            write_to_guest(&mut caller, snapshot.as_ref(), ret_ptr, ret_size)
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
            if caller.data().frame.phase != WasmPhase::RequestBody {
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
            let work = request_body_mutation_work(caller.data().frame.request_body.len(), &range, len as usize);
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
            if caller.data().frame.read_only {
                return STATUS_UNIMPLEMENTED;
            }
            // Nonpositive sizes intentionally ignore their pointer. For
            // positive inputs preserve body-range validation before headers.
            let body_profile = if body_size > 0 {
                match profile_guest_range(&mut caller, body_ptr, body_size) {
                    Ok(profile) => Some(profile),
                    Err(status) => return status,
                }
            } else {
                None
            };
            let headers_profile = if headers_size > 0 {
                let guest_result = profile_guest_memory_range(&mut caller, headers_ptr, headers_size);
                let Ok((memory, guest)) = guest_result else {
                    return guest_result.expect_err("failed guest-range profiling must carry an ABI status");
                };
                let bytes_result = guest_bytes(&caller, memory, &guest);
                let Ok(bytes) = bytes_result else {
                    return bytes_result.expect_err("failed guest-memory access must carry an ABI status");
                };
                match scan_serialized_map(bytes) {
                    Ok(map) => Some(GuestMapProfile { guest, map }),
                    // Compatibility: malformed local-response header maps are
                    // accepted as an empty header list.
                    Err(STATUS_BAD_ARGUMENT) => None,
                    Err(status) => return status,
                }
            } else {
                None
            };
            let mut allocation = AllocationProfile::default();
            if let Some(profile) = &body_profile {
                allocation = match allocation.add_bytes(profile.len()) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
            }
            if let Some(profile) = &headers_profile {
                allocation =
                    match decoded_map_allocation(profile.map).and_then(|headers| allocation.add_bytes(headers.bytes)) {
                        Ok(profile) => profile,
                        Err(status) => return status,
                    };
            }
            if let Some(existing) = caller.data().frame.local_response.as_ref() {
                allocation = match local_response_allocation(existing)
                    .and_then(|existing| allocation.add_bytes(existing.bytes))
                {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
            }
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            let body = match &body_profile {
                Some(profile) => match copy_profiled_guest(&mut caller, profile) {
                    Ok(body) => body,
                    Err(status) => return status,
                },
                None => Vec::new(),
            };
            let headers = match &headers_profile {
                Some(profile) => match decode_profiled_guest_map(&mut caller, profile) {
                    Ok(headers) => headers,
                    Err(status) => return status,
                },
                None => Vec::new(),
            };
            let status = if (200..=599).contains(&status) {
                status as u16
            } else {
                500
            };
            // Replacement state (a single Option), not an accumulator: repeated
            // calls overwrite rather than grow, so its peak is one body, bounded
            // by the `ResourceLimiter`-capped guest linear memory. Not charged
            // against the append-only ABI retention budget.
            caller.data_mut().frame.local_response = Some(LocalResponse { status, body, headers });
            STATUS_OK
        },
    )?;

    linker.func_wrap(
        host::MODULE,
        host::PROXY_GET_PROPERTY,
        |mut caller: Caller<'_, HostState>, pp: i32, pl: i32, ret_ptr: i32, ret_size: i32| -> i32 {
            let path_profile = match profile_property_path(&mut caller, pp, pl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let key_allocation = match AllocationProfile::default().add_bytes(path_profile.decoded_len) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            if let Err(status) = caller.data().check_transient_allocation(key_allocation) {
                return status;
            }
            let key = match decode_property_path(&mut caller, &path_profile) {
                Ok(key) => key,
                Err(status) => return status,
            };
            let value = {
                let Some(value) = caller.data().frame.properties.get(&key) else {
                    return STATUS_NOT_FOUND;
                };
                let allocation = match key_allocation.add_bytes(value.len()) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
                if let Err(status) = caller.data().check_transient_allocation(allocation) {
                    return status;
                }
                match try_clone_string(value) {
                    Ok(value) => value,
                    Err(status) => return status,
                }
            };
            // The owned value survives allocator reentry; the lookup key is done.
            drop(key);
            write_to_guest(&mut caller, value.as_bytes(), ret_ptr, ret_size)
        },
    )?;

    linker.func_wrap(
        host::MODULE,
        host::PROXY_SET_PROPERTY,
        |mut caller: Caller<'_, HostState>, pp: i32, pl: i32, vp: i32, vl: i32| -> i32 {
            let path_profile = match profile_property_path(&mut caller, pp, pl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let value_profile = match profile_guest_string(&mut caller, vp, vl) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let allocation = match AllocationProfile::default()
                .add_bytes(path_profile.decoded_len)
                .and_then(|profile| profile.add_bytes(value_profile.decoded_len))
                .and_then(|profile| profile.add_bytes(conservative_hash_table_allocation::<(String, String)>(1)?.bytes))
            {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            if let Err(status) = caller.data().check_transient_allocation(allocation) {
                return status;
            }
            let checked_charge = match caller.data().checked_abi_charge(allocation.bytes) {
                Ok(charge) => charge,
                Err(status) => return status,
            };
            let key = match decode_property_path(&mut caller, &path_profile) {
                Ok(key) => key,
                Err(status) => return status,
            };
            let value = match decode_profiled_guest_string(&mut caller, &value_profile) {
                Ok(value) => value,
                Err(status) => return status,
            };
            let state = caller.data_mut();
            if !state.frame.properties.contains_key(&key) && state.frame.properties.try_reserve(1).is_err() {
                return STATUS_INTERNAL_FAILURE;
            }
            state.commit_abi_charge(checked_charge);
            state.frame.properties.insert(key, value);
            STATUS_OK
        },
    )?;

    // Legacy 0.1 configuration accessor (some SDKs call it from on_configure).
    linker.func_wrap(
        host::MODULE,
        host::PROXY_GET_CONFIGURATION,
        |mut caller: Caller<'_, HostState>, ret_ptr: i32, ret_size: i32| -> i32 {
            // Keep an independent owner across guest allocator reentry without copying the payload.
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

    // Shared data (plugin-runtime KV with CAS). Process-local: shared across
    // all pooled VMs of one ModuleEntry, freed with its last holder.
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
            let key_profile = match profile_guest_string(&mut caller, key_ptr, key_size) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            let Ok(allocation) = AllocationProfile::default()
                .add_bytes(key_profile.decoded_len)
                .and_then(|profile| profile.add_bytes(MAX_SHARED_VALUE_BYTES))
            else {
                return STATUS_BAD_ARGUMENT;
            };
            if allocation.bytes > caller.data().allocation_limits.transient_bytes {
                return STATUS_BAD_ARGUMENT;
            }
            let key = match decode_profiled_guest_string(&mut caller, &key_profile) {
                Ok(key) => key,
                Err(status) => return status,
            };
            let Some((value, cas)) = caller.data().shared_data.get(&key) else {
                return STATUS_NOT_FOUND;
            };
            // SharedData returned an independent value/CAS snapshot.
            drop(key);
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
            let key_profile = match profile_guest_string(&mut caller, key_ptr, key_size) {
                Ok(profile) => profile,
                Err(status) => return status,
            };
            // Null value means delete. A non-null pointer with a zero size is
            // an ordinary empty-value write and still validates its range.
            let value_profile = if value_ptr == 0 && value_size == 0 {
                None
            } else {
                let profile = match profile_guest_range(&mut caller, value_ptr, value_size) {
                    Ok(profile) => profile,
                    Err(status) => return status,
                };
                // Preserve precedence: validate the guest range before the
                // shared-data 64 KiB semantic cap.
                if profile.len() > MAX_SHARED_VALUE_BYTES {
                    return STATUS_BAD_ARGUMENT;
                }
                Some(profile)
            };
            let value_len = value_profile.as_ref().map_or(0, GuestRangeProfile::len);
            let Ok(allocation) = AllocationProfile::default()
                // The SharedData map clones the decoded key on insertion.
                .add_bytes(key_profile.decoded_len)
                .and_then(|profile| profile.add_bytes(key_profile.decoded_len))
                .and_then(|profile| profile.add_bytes(value_len))
                .and_then(|profile| profile.add_bytes(ENTRY_OVERHEAD_BYTES))
            else {
                return STATUS_BAD_ARGUMENT;
            };
            if allocation.bytes > caller.data().allocation_limits.transient_bytes {
                return STATUS_BAD_ARGUMENT;
            }
            let key = match decode_profiled_guest_string(&mut caller, &key_profile) {
                Ok(key) => key,
                Err(status) => return status,
            };
            let value = match &value_profile {
                Some(profile) => match copy_profiled_guest(&mut caller, profile) {
                    Ok(value) => Some(value),
                    Err(status) => return status,
                },
                None => None,
            };
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
                caller.data_mut().frame.continue_requested = true;
            }
            STATUS_OK
        },
    )?;
    linker.func_wrap(
        host::MODULE,
        host::PROXY_CONTINUE_REQUEST,
        |mut caller: Caller<'_, HostState>| -> i32 {
            caller.data_mut().frame.continue_requested = true;
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
#[path = "../../../../tests/unit/plugins/http/wasm/host_abi_test.rs"]
mod tests;
