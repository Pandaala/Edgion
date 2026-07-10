//! proxy-wasm ABI v0.2 host bridge (headers + local response subset).
//!
//! Design: host functions never touch the PluginSession. The plugin snapshots
//! request/response headers into `HostState` before invoking the guest, the
//! guest mutates the snapshot (so it can read back its own writes) while ops
//! are also queued, and the plugin drains `pending_ops` onto the session after
//! the guest returns. Buffers returned to the guest are allocated by calling
//! the guest's own `proxy_on_memory_allocate` export.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use wasmtime::{Caller, Extern, Linker};

pub const MAP_HTTP_REQUEST_HEADERS: i32 = 0;
pub const MAP_HTTP_RESPONSE_HEADERS: i32 = 2;

pub const BUF_VM_CONFIGURATION: i32 = 6;
pub const BUF_PLUGIN_CONFIGURATION: i32 = 7;

pub const STATUS_OK: i32 = 0;
pub const STATUS_NOT_FOUND: i32 = 1;
pub const STATUS_BAD_ARGUMENT: i32 = 2;
pub const STATUS_INVALID_MEMORY_ACCESS: i32 = 6;
pub const STATUS_INTERNAL_FAILURE: i32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WasmPhase {
    #[default]
    RequestHeaders,
    ResponseHeaders,
}

#[derive(Debug, Clone)]
pub enum HeaderOp {
    Set { map: i32, name: String, value: String },
    Add { map: i32, name: String, value: String },
    Remove { map: i32, name: String },
    ReplaceAll { map: i32, pairs: Vec<(String, String)> },
}

#[derive(Debug, Clone)]
pub struct LocalResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
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
}

impl HostState {
    pub fn new(vm_config: Vec<u8>, plugin_config: Vec<u8>) -> Self {
        HostState {
            vm_config,
            plugin_config,
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
    }

    fn map_mut(&mut self, map: i32) -> Option<&mut Vec<(String, String)>> {
        match map {
            MAP_HTTP_REQUEST_HEADERS => Some(&mut self.request_headers),
            MAP_HTTP_RESPONSE_HEADERS => Some(&mut self.response_headers),
            _ => None,
        }
    }

    fn map_ref(&self, map: i32) -> Option<&Vec<(String, String)>> {
        match map {
            MAP_HTTP_REQUEST_HEADERS => Some(&self.request_headers),
            MAP_HTTP_RESPONSE_HEADERS => Some(&self.response_headers),
            _ => None,
        }
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
    let mut buf = vec![0u8; len as usize];
    mem.read(&mut *caller, ptr as usize, &mut buf)
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
                    tracing::debug!(target: "wasm_guest", level, %msg, "guest log");
                    caller.data_mut().log_lines.push((level, msg));
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
            let pairs = match caller.data().map_ref(map) {
                Some(p) => p.clone(),
                None => return STATUS_BAD_ARGUMENT,
            };
            let encoded = encode_map(&pairs);
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
            let value = match caller.data().map_ref(map) {
                Some(pairs) => pairs
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(&key))
                    .map(|(_, v)| v.clone()),
                None => return STATUS_BAD_ARGUMENT,
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
            let Some(pairs) = state.map_mut(map) else {
                return STATUS_BAD_ARGUMENT;
            };
            *pairs = new_pairs.clone();
            state.pending_ops.push(HeaderOp::ReplaceAll { map, pairs: new_pairs });
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
                _ => return STATUS_NOT_FOUND,
            };
            let start = start.max(0) as usize;
            if start >= data.len() && !data.is_empty() {
                return STATUS_BAD_ARGUMENT;
            }
            let slice = slice_buffer(&data, start, max_size.max(0) as usize);
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
            caller.data_mut().properties.insert(key, value);
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

    // Accepted no-ops for the v1 subset.
    linker.func_wrap(
        "env",
        "proxy_set_effective_context",
        |_: Caller<'_, HostState>, _ctx: i32| -> i32 { STATUS_OK },
    )?;
    linker.func_wrap(
        "env",
        "proxy_set_tick_period_milliseconds",
        |_: Caller<'_, HostState>, _ms: i32| -> i32 { STATUS_OK },
    )?;
    linker.func_wrap("env", "proxy_done", |_: Caller<'_, HostState>| -> i32 { STATUS_OK })?;
    linker.func_wrap(
        "env",
        "proxy_continue_stream",
        |_: Caller<'_, HostState>, _stream: i32| -> i32 { STATUS_OK },
    )?;
    linker.func_wrap("env", "proxy_continue_request", |_: Caller<'_, HostState>| -> i32 {
        STATUS_OK
    })?;
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
}
