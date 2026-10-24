#![allow(clippy::field_reassign_with_default)]

use super::*;
use crate::plugins::http::wasm::shared_data::{SharedData, MAX_SHARED_TOTAL_BYTES};
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

#[test]
fn map_encoding_preserves_literal_wire_layout() {
    let cases = [
        (Vec::new(), vec![0, 0, 0, 0]),
        (
            vec![(String::new(), String::new())],
            vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        ),
        (
            vec![("x".to_owned(), "é".to_owned()), ("x".to_owned(), String::new())],
            vec![
                2, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, b'x', 0, 0xc3, 0xa9, 0, b'x', 0, 0,
            ],
        ),
    ];
    for (pairs, expected) in cases {
        let actual = encode_map(&pairs);
        assert_eq!(actual, expected);
        assert_eq!(decode_map(&actual).unwrap(), pairs);
    }
}

#[test]
fn map_encoding_reserves_only_serialized_bytes() {
    // Freeze the previous reservation strategy as an allocation oracle.
    fn previous_encode(pairs: &[(String, String)]) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + pairs.iter().map(|(k, v)| k.len() + v.len() + 10).sum::<usize>());
        out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
        for (key, value) in pairs {
            out.extend_from_slice(&(key.len() as u32).to_le_bytes());
            out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        }
        for (key, value) in pairs {
            out.extend_from_slice(key.as_bytes());
            out.push(0);
            out.extend_from_slice(value.as_bytes());
            out.push(0);
        }
        out
    }

    for count in [0, 1, 2, 64, 1024] {
        let pairs = vec![("x-field".to_owned(), "é".to_owned()); count];
        let (previous, previous_stats) =
            crate::test_allocation_counter::count_allocation_stats(|| previous_encode(&pairs));
        let (actual, actual_stats) = crate::test_allocation_counter::count_allocation_stats(|| encode_map(&pairs));
        assert_eq!(actual, previous);
        assert_eq!(actual_stats.calls, 1);
        assert_eq!(actual_stats.calls, previous_stats.calls);
        assert_eq!(actual_stats.requested_bytes, actual.len());
        assert_eq!(actual_stats.requested_bytes + 12, previous_stats.requested_bytes);
    }
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
    assert_eq!(buffer_range(0, 3, 10), 0..0);
}

#[test]
fn slice_buffer_normal_cases() {
    let data = b"hello world";
    assert_eq!(&data[buffer_range(data.len(), 0, 5)], b"hello");
    assert_eq!(&data[buffer_range(data.len(), 6, 100)], b"world");
    assert_eq!(&data[buffer_range(data.len(), 100, 5)], b"");
    assert_eq!(buffer_range(data.len(), 6, usize::MAX), 6..data.len());
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
        .frame
        .request_headers
        .iter()
        .any(|(k, v)| k == "x-wat-added" && v == "yes"));
    assert!(matches!(
        state.frame.pending_ops.as_slice(),
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
    let lr = store.data().frame.local_response.as_ref().expect("local response set");
    assert_eq!(lr.status, 403);
    assert_eq!(lr.body, b"denied");
}

#[test]
fn guest_local_response_uses_only_final_statuses() {
    let (mut store, instance) = instantiate_allocation_guest();
    let local = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "local")
        .unwrap();
    for status in [100, 101, 199, 200, 599, 600] {
        assert_eq!(local.call(&mut store, (status, 0, 0, 0, 0)).unwrap(), STATUS_OK);
        let expected = if (200..=599).contains(&status) {
            status as u16
        } else {
            500
        };
        assert_eq!(store.data().frame.local_response.as_ref().unwrap().status, expected);
    }
}

/// A single payload larger than the byte budget is rejected, and the
/// rejection charges nothing (no partial mutation of the counters).
#[test]
fn charge_abi_rejects_over_byte_budget_atomically() {
    let mut s = HostState::default();
    assert_eq!(s.charge_abi(MAX_RETAINED_ABI_BYTES), Err(STATUS_INTERNAL_FAILURE));
    assert_eq!(s.frame.retained_bytes, 0);
    assert_eq!(s.frame.retained_ops, 0);
    // A payload that fits is charged with the fixed per-op overhead.
    assert_eq!(s.charge_abi(10), Ok(()));
    assert_eq!(s.frame.retained_bytes, 10 + ABI_OP_OVERHEAD_BYTES);
    assert_eq!(s.frame.retained_ops, 1);
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
    assert_eq!(s.frame.retained_ops, MAX_RETAINED_ABI_OPS);
    assert!(s.frame.retained_bytes < MAX_RETAINED_ABI_BYTES);
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
    let callout_cost = store.data().frame.retained_bytes;
    store
        .data_mut()
        .begin_phase(WasmPhase::RequestHeaders, Vec::new(), Vec::new(), HashMap::new());
    store
        .data_mut()
        .arm_dispatch(Arc::from(vec!["http://example.com:80".to_string()]), 1000);
    let preflight_cost = profile_callout_allocation(b"http://example.com", &[0, 0, 0, 0], 0)
        .unwrap()
        .payload_bytes
        + ABI_OP_OVERHEAD_BYTES;
    let admission_cost = callout_cost.max(preflight_cost);
    let retained_before = MAX_RETAINED_ABI_BYTES - admission_cost;
    store.data_mut().frame.retained_bytes = retained_before;

    assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_OK);
    let state = store.data();
    assert_eq!(state.frame.retained_bytes, retained_before + callout_cost);
    assert_eq!(state.frame.retained_ops, 1);
    assert_eq!(state.frame.pending_callouts.len(), 1);
    assert_eq!(state.frame.callouts_dispatched, 1);
    assert_eq!(state.last_callout_token, 2);

    assert_eq!(dispatch.call(&mut store, 132).unwrap(), STATUS_INTERNAL_FAILURE);
    let state = store.data();
    assert_eq!(state.frame.retained_bytes, retained_before + callout_cost);
    assert_eq!(state.frame.retained_ops, 1);
    assert_eq!(state.frame.pending_callouts.len(), 1);
    assert_eq!(state.frame.callouts_dispatched, 1);
    assert_eq!(state.last_callout_token, 2);
}

#[test]
fn callout_map_decoding_preserves_permissive_wire_and_publication() {
    let invalid = [b'a', 0xff, b'b'];
    let pairs: &[(&[u8], &[u8])] = &[
        (b":path", b"first"),
        (b"X-Field", &invalid),
        (b"X-Field", b"second"),
        (b":path", b"last"),
    ];
    let mut encoded = encode_raw_map(pairs);
    let mut offset = 4 + pairs.len() * 8;
    for (key, value) in pairs {
        offset += key.len();
        encoded[offset] = b'!';
        offset += 1 + value.len();
        encoded[offset] = b'?';
        offset += 1;
    }
    encoded.extend_from_slice(b"trailing");
    let (mut store, instance) = instantiate_callout_guest();
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    memory.write(&mut store, 1024, &encoded).unwrap();
    let dispatch = instance
        .get_typed_func::<(i32, i32, i32), i32>(&mut store, "dispatch_custom_headers")
        .unwrap();
    assert_eq!(
        dispatch.call(&mut store, (1024, encoded.len() as i32, 512)).unwrap(),
        STATUS_OK
    );
    let mut token = [0; 4];
    memory.read(&store, 512, &mut token).unwrap();
    assert_eq!(u32::from_le_bytes(token), 1);
    let state = store.data();
    assert_eq!(state.frame.pending_callouts.len(), 1);
    let pending = &state.frame.pending_callouts[0];
    assert_eq!(pending.path, "/last");
    assert_eq!(pending.method, "GET");
    assert_eq!(
        pending.headers,
        vec![
            ("X-Field".to_owned(), String::from_utf8_lossy(&invalid).into_owned()),
            ("X-Field".to_owned(), "second".to_owned()),
        ]
    );
    assert_eq!(state.frame.callouts_dispatched, 1);
    assert_eq!(state.last_callout_token, 1);
    assert_eq!(state.frame.retained_ops, 1);
    assert_eq!(
        state.frame.retained_bytes,
        pending_callout_retained_payload(pending).unwrap() + ABI_OP_OVERHEAD_BYTES
    );
}

#[test]
fn malformed_callout_map_count_is_bad_argument_without_state_change() {
    let (mut store, instance) = instantiate_callout_guest();
    let dispatch = instance
        .get_typed_func::<i32, i32>(&mut store, "dispatch_bad_map")
        .unwrap();
    assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_BAD_ARGUMENT);
    let state = store.data();
    assert_eq!(state.frame.retained_bytes, 0);
    assert_eq!(state.frame.retained_ops, 0);
    assert!(state.frame.pending_callouts.is_empty());
    assert_eq!(state.frame.callouts_dispatched, 0);
    assert_eq!(state.last_callout_token, 0);
}

#[test]
fn final_callout_capacity_charge_rejects_atomically_after_raw_preflight() {
    let (mut store, instance) = instantiate_callout_guest();
    let dispatch = instance.get_typed_func::<i32, i32>(&mut store, "dispatch").unwrap();
    let raw_preflight_cost = 18 + 4 + ABI_OP_OVERHEAD_BYTES;
    let retained_before = MAX_RETAINED_ABI_BYTES - raw_preflight_cost;
    store.data_mut().frame.retained_bytes = retained_before;
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    memory.write(&mut store, 128, &[0xA5; 4]).unwrap();

    assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_INTERNAL_FAILURE);
    let mut token_bytes = [0u8; 4];
    memory.read(&store, 128, &mut token_bytes).unwrap();
    assert_eq!(token_bytes, [0xA5; 4]);
    let state = store.data();
    assert_eq!(state.frame.retained_bytes, retained_before);
    assert_eq!(state.frame.retained_ops, 0);
    assert!(state.frame.pending_callouts.is_empty());
    assert_eq!(state.frame.callouts_dispatched, 0);
    assert_eq!(state.last_callout_token, 0);
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
    assert_eq!(state.frame.retained_bytes, 0);
    assert_eq!(state.frame.retained_ops, 0);
    assert!(state.frame.pending_callouts.is_empty());
    assert_eq!(state.frame.callouts_dispatched, 0);
    assert_eq!(state.last_callout_token, 0);
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

    state.frame.retained_bytes = MAX_RETAINED_ABI_BYTES;
    let large_input = callout_input_payload_bytes(i32::MAX, i32::MAX, i32::MAX).unwrap();
    assert_eq!(
        state.checked_abi_charge(large_input),
        Err(STATUS_INTERNAL_FAILURE),
        "length preflight rejects before any guest-memory copy"
    );

    state.begin_phase(WasmPhase::Tick, Vec::new(), Vec::new(), HashMap::new());
    assert_eq!(state.frame.retained_bytes, 0);
    assert_eq!(state.frame.retained_ops, 0);
}

#[test]
fn callout_preflight_rejects_lossy_utf8_expansion_without_retained_state_change() {
    let upstream = b"http://example.com";
    let invalid_name = [0xFF; 16];
    let headers = encode_raw_map(&[(&invalid_name, b"value")]);
    let raw_payload = upstream.len() + headers.len();
    let allocation_payload = profile_callout_allocation(upstream, &headers, 0).unwrap().payload_bytes;
    let valid_name = [b'x'; 16];
    let valid_headers = encode_raw_map(&[(&valid_name, b"value")]);
    let valid_allocation_payload = profile_callout_allocation(upstream, &valid_headers, 0)
        .unwrap()
        .payload_bytes;
    assert_eq!(
        allocation_payload - valid_allocation_payload,
        invalid_name.len() * 2,
        "each invalid byte expands from one raw byte to a three-byte replacement"
    );

    let state = HostState {
        frame: CallbackFrame {
            retained_bytes: MAX_RETAINED_ABI_BYTES - raw_payload - ABI_OP_OVERHEAD_BYTES,
            ..Default::default()
        },
        ..Default::default()
    };
    let retained_before = state.frame.retained_bytes;
    assert!(state.checked_abi_charge(raw_payload).is_ok());
    assert_eq!(
        state.checked_abi_charge(allocation_payload),
        Err(STATUS_INTERNAL_FAILURE)
    );
    assert_eq!(state.frame.retained_bytes, retained_before);
    assert_eq!(state.frame.retained_ops, 0);
    assert!(state.frame.pending_callouts.is_empty());
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
    store.data_mut().frame.retained_bytes = retained_before;
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
    assert_eq!(state.frame.retained_bytes, retained_before);
    assert_eq!(state.frame.retained_ops, 0);
    assert!(state.frame.pending_callouts.is_empty());
    assert_eq!(state.frame.callouts_dispatched, 0);
    assert_eq!(state.last_callout_token, 0);
}

#[test]
fn callout_preflight_rejects_high_pair_count_without_retained_state_change() {
    let upstream = b"http://example.com";
    let empty_pairs = vec![(&[][..], &[][..]); 128];
    let headers = encode_raw_map(&empty_pairs);
    let raw_payload = upstream.len() + headers.len();
    let allocation_payload = profile_callout_allocation(upstream, &headers, 0).unwrap().payload_bytes;
    assert!(
        allocation_payload >= raw_payload + 128 * 2 * std::mem::size_of::<(String, String)>(),
        "both concurrently-live pair Vec capacities must be preflighted"
    );

    let state = HostState {
        frame: CallbackFrame {
            retained_bytes: MAX_RETAINED_ABI_BYTES - raw_payload - ABI_OP_OVERHEAD_BYTES,
            ..Default::default()
        },
        ..Default::default()
    };
    let retained_before = state.frame.retained_bytes;
    assert!(state.checked_abi_charge(raw_payload).is_ok());
    assert_eq!(
        state.checked_abi_charge(allocation_payload),
        Err(STATUS_INTERNAL_FAILURE)
    );
    assert_eq!(state.frame.retained_bytes, retained_before);
    assert_eq!(state.frame.retained_ops, 0);
    assert!(state.frame.pending_callouts.is_empty());
}

/// `begin_phase` must clear the staged request body between phases so a
/// stale buffer-mode body never leaks into a later phase's `HostState`.
#[test]
fn begin_phase_clears_request_body() {
    let mut state = HostState::default();
    state.frame.request_body = vec![1, 2, 3];
    state.frame.request_body_limit = 9;
    state.frame.request_body_modified = true;
    state.frame.request_body_mutation_work = 123;
    state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
    assert!(state.frame.request_body.is_empty());
    assert_eq!(state.frame.request_body_limit, 0);
    assert!(!state.frame.request_body_modified);
    assert_eq!(state.frame.request_body_mutation_work, 0);
    assert_eq!(state.frame.phase, WasmPhase::RequestBody);
}

/// Request snapshots own only the selected range and survive source mutation.
#[test]
fn buffer_select_preserves_owned_request_body() {
    let mut state = HostState::default();
    state.frame.request_body = vec![b'a'; 1024 * 1024];
    state.frame.request_body[42..45].copy_from_slice(b"XYZ");
    let selected = select_buffer(&state, BUF_HTTP_REQUEST_BODY)
        .unwrap()
        .snapshot(42, 3)
        .unwrap();
    state.frame.request_body.clear();
    assert_eq!(selected.as_ref(), b"XYZ");
    let BufferSnapshot::Owned(bytes) = selected else {
        panic!("request range must be independently owned");
    };
    assert_eq!(bytes.len(), 3);
    assert!(select_buffer(&state, 999).is_none());
}

#[test]
fn response_buffer_selection_shares_chunk_backing_storage() {
    let chunk = Bytes::from(vec![1, 2, 3, 4]);
    let mut state = HostState::default();
    state.frame.body_chunk = chunk.clone();
    let selected = select_buffer(&state, BUF_HTTP_RESPONSE_BODY)
        .unwrap()
        .snapshot(1, 2)
        .unwrap();
    state.frame.body_chunk = Bytes::new();
    assert_eq!(selected.as_ref(), &[2, 3]);
    assert_eq!(selected.as_ref().as_ptr(), chunk[1..].as_ptr());
    assert!(matches!(selected, BufferSnapshot::Shared(_)));
}

#[test]
fn buffer_snapshots_cover_vec_sources_and_empty_callout() {
    let mut state = HostState::default();
    state.vm_config = b"vm-config".to_vec();
    state.plugin_config = b"plugin-config".to_vec();
    state.frame.request_body = b"request".to_vec();
    state.frame.callout_response = Some(CalloutResponse {
        body: b"callout".to_vec(),
        ..Default::default()
    });
    let snapshots: Vec<_> = [
        BUF_VM_CONFIGURATION,
        BUF_PLUGIN_CONFIGURATION,
        BUF_HTTP_REQUEST_BODY,
        BUF_HTTP_CALL_RESPONSE_BODY,
    ]
    .into_iter()
    .map(|id| select_buffer(&state, id).unwrap().snapshot(1, 2).unwrap())
    .collect();
    for id in [
        BUF_VM_CONFIGURATION,
        BUF_PLUGIN_CONFIGURATION,
        BUF_HTTP_REQUEST_BODY,
        BUF_HTTP_CALL_RESPONSE_BODY,
    ] {
        let source = select_buffer(&state, id).unwrap();
        assert!(matches!(source.snapshot(usize::MAX, 0), Err(STATUS_BAD_ARGUMENT)));
    }
    state.vm_config.clear();
    state.plugin_config.clear();
    state.frame.request_body.clear();
    state.frame.callout_response = None;
    for (snapshot, expected) in snapshots.iter().zip([b"m-", b"lu", b"eq", b"al"]) {
        assert_eq!(snapshot.as_ref(), expected);
        assert!(matches!(snapshot, BufferSnapshot::Owned(bytes) if bytes.len() == 2));
    }
    let empty = select_buffer(&state, BUF_HTTP_CALL_RESPONSE_BODY)
        .unwrap()
        .snapshot(usize::MAX, usize::MAX)
        .unwrap();
    assert!(empty.as_ref().is_empty());
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
    assert!(state.frame.log_lines.len() >= 30 && state.frame.log_lines.len() <= 34);
    assert!(state.frame.retained_bytes <= MAX_RETAINED_ABI_BYTES);
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
    assert!(!state.frame.pending_ops.is_empty());
    assert_eq!(state.frame.request_headers.len(), state.frame.pending_ops.len());
    assert!(state.frame.request_headers.len() < 5000); // stopped by budget, not the cap
}

#[test]
fn request_body_range_mutations_are_atomic_and_bounded() {
    let mut state = HostState::default();
    state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
    state.frame.request_body = b"abcdef".to_vec();
    state.frame.request_body_limit = 12;

    assert_eq!(replace_request_body_buffer(&mut state, 0, 0, Vec::new()), STATUS_OK);
    assert!(!state.frame.request_body_modified, "an empty insertion is a no-op");
    assert_eq!(
        replace_request_body_buffer(&mut state, 1, 2, b"XYZ".to_vec()),
        STATUS_OK
    );
    assert_eq!(state.frame.request_body, b"aXYZdef");
    assert_eq!(replace_request_body_buffer(&mut state, 1, 0, b"+".to_vec()), STATUS_OK);
    assert_eq!(state.frame.request_body, b"a+XYZdef");
    let end = state.frame.request_body.len();
    assert_eq!(
        replace_request_body_buffer(&mut state, end, 0, b"!".to_vec()),
        STATUS_OK
    );
    assert_eq!(state.frame.request_body, b"a+XYZdef!");
    assert_eq!(replace_request_body_buffer(&mut state, 2, 3, Vec::new()), STATUS_OK);
    assert_eq!(state.frame.request_body, b"a+def!");

    let before = state.frame.request_body.clone();
    assert_eq!(
        replace_request_body_buffer(&mut state, before.len() + 1, 0, b"x".to_vec()),
        STATUS_BAD_ARGUMENT
    );
    assert_eq!(state.frame.request_body, before);
    assert_eq!(
        replace_request_body_buffer(&mut state, 0, 0, vec![b'x'; 20]),
        STATUS_INTERNAL_FAILURE
    );
    assert_eq!(state.frame.request_body, before);

    // Body replacement payload is not append-only retained ABI memory, so
    // it uses the separate mutation-work counter. Even near the generic
    // retained-byte budget, one equal-size rewrite succeeds.
    state.frame.retained_bytes = MAX_RETAINED_ABI_BYTES - ABI_OP_OVERHEAD_BYTES;
    let replacement = vec![b'z'; state.frame.request_body.len()];
    let body_len = state.frame.request_body.len();
    let work_before = state.frame.request_body_mutation_work;
    assert_eq!(
        replace_request_body_buffer(&mut state, 0, body_len, replacement),
        STATUS_OK
    );
    assert_eq!(state.frame.retained_bytes, MAX_RETAINED_ABI_BYTES);
    assert_eq!(state.frame.request_body_mutation_work, work_before + body_len);
    assert!(state.frame.request_body_modified);
}

#[test]
fn request_body_repeated_full_rewrites_stop_at_work_budget_atomically() {
    let mut state = HostState::default();
    state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
    state.frame.request_body = b"abcd".to_vec();
    state.frame.request_body_limit = 4;
    state.frame.request_body_mutation_work = request_body_mutation_work_limit(state.frame.request_body_limit) - 4;

    assert_eq!(
        replace_request_body_buffer(&mut state, 0, 4, b"WXYZ".to_vec()),
        STATUS_OK
    );
    assert_eq!(state.frame.request_body, b"WXYZ");
    assert_eq!(
        state.frame.request_body_mutation_work,
        request_body_mutation_work_limit(state.frame.request_body_limit)
    );

    let retained_bytes = state.frame.retained_bytes;
    let retained_ops = state.frame.retained_ops;
    assert_eq!(
        replace_request_body_buffer(&mut state, 0, 4, b"nope".to_vec()),
        STATUS_INTERNAL_FAILURE
    );
    assert_eq!(state.frame.request_body, b"WXYZ");
    assert_eq!(
        state.frame.request_body_mutation_work,
        request_body_mutation_work_limit(state.frame.request_body_limit)
    );
    assert_eq!(state.frame.retained_bytes, retained_bytes);
    assert_eq!(state.frame.retained_ops, retained_ops);
}

#[test]
fn request_body_length_change_charges_shifted_tail_work() {
    let mut state = HostState::default();
    state.begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
    state.frame.request_body = b"abcd".to_vec();
    state.frame.request_body_limit = 4;
    state.frame.request_body_mutation_work = request_body_mutation_work_limit(state.frame.request_body_limit) - 3;

    // Deleting the first byte moves the remaining three-byte tail.
    assert_eq!(replace_request_body_buffer(&mut state, 0, 1, Vec::new()), STATUS_OK);
    assert_eq!(state.frame.request_body, b"bcd");
    assert_eq!(
        state.frame.request_body_mutation_work,
        request_body_mutation_work_limit(state.frame.request_body_limit)
    );

    // A second front deletion would move another two-byte tail and must
    // be rejected without changing either the body or the counters.
    let retained_bytes = state.frame.retained_bytes;
    let retained_ops = state.frame.retained_ops;
    assert_eq!(
        replace_request_body_buffer(&mut state, 0, 1, Vec::new()),
        STATUS_INTERNAL_FAILURE
    );
    assert_eq!(state.frame.request_body, b"bcd");
    assert_eq!(state.frame.retained_bytes, retained_bytes);
    assert_eq!(state.frame.retained_ops, retained_ops);
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
    store.data_mut().frame.request_body = b"abcdef".to_vec();
    store.data_mut().frame.request_body_limit = 16;
    assert_eq!(
        f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 1, 2, 0, 3)).unwrap(),
        STATUS_OK
    );
    assert_eq!(store.data().frame.request_body, b"aXYZdef");
    assert!(store.data().frame.request_body_modified);
    assert_eq!(
        f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 99, 0, i32::MAX, 3)).unwrap(),
        STATUS_BAD_ARGUMENT,
        "an impossible range must be rejected before guest-memory copying"
    );
    assert_eq!(
        f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 0, 0, i32::MAX, 3)).unwrap(),
        STATUS_INVALID_MEMORY_ACCESS
    );
    let mutation_limit = request_body_mutation_work_limit(store.data().frame.request_body_limit);
    store.data_mut().frame.request_body_mutation_work = mutation_limit;
    let body_before = store.data().frame.request_body.clone();
    assert_eq!(
        f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 0, 3, i32::MAX, 3)).unwrap(),
        STATUS_INTERNAL_FAILURE,
        "an exhausted work budget must reject before guest-memory copying"
    );
    assert_eq!(store.data().frame.request_body, body_before);
    assert_eq!(
        f.call(&mut store, (BUF_HTTP_REQUEST_BODY, 0, 0, 0, -1)).unwrap(),
        STATUS_BAD_ARGUMENT
    );
    assert_eq!(
        f.call(&mut store, (BUF_HTTP_RESPONSE_BODY, -1, -1, i32::MAX, -1))
            .unwrap(),
        STATUS_UNIMPLEMENTED
    );
    for populated in [false, true] {
        store.data_mut().vm_config = if populated { b"vm".to_vec() } else { Vec::new() };
        store.data_mut().plugin_config = if populated { b"plugin".to_vec() } else { Vec::new() };
        store.data_mut().frame.callout_response = populated.then(|| CalloutResponse {
            body: b"callout".to_vec(),
            ..Default::default()
        });
        for id in [
            BUF_VM_CONFIGURATION,
            BUF_PLUGIN_CONFIGURATION,
            BUF_HTTP_CALL_RESPONSE_BODY,
        ] {
            assert_eq!(
                f.call(&mut store, (id, -1, -1, i32::MAX, -1)).unwrap(),
                STATUS_BAD_ARGUMENT
            );
        }
    }
    assert_eq!(
        f.call(&mut store, (999, -1, -1, i32::MAX, -1)).unwrap(),
        STATUS_NOT_FOUND
    );
}

/// The allocator records every invocation, can trap, or can reenter SET.
const GET_BUFFER_BYTES_GUEST: &str = r#"
    (module
      (import "env" "proxy_get_buffer_bytes"
        (func $get (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_set_buffer_bytes"
        (func $set (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $mode (mut i32) (i32.const 0))
      (data (i32.const 32) "XYZ")
      (func (export "allocator_mode") (param i32) (global.set $mode (local.get 0)))
      (func (export "proxy_on_memory_allocate") (param $size i32) (result i32)
        (i32.store (i32.const 16) (i32.add (i32.load (i32.const 16)) (i32.const 1)))
        (i32.store (i32.const 20) (local.get $size))
        (if (i32.eq (global.get $mode) (i32.const 1)) (then unreachable))
        (if (i32.eq (global.get $mode) (i32.const 2))
          (then
            (i32.store (i32.const 24)
              (call $set (i32.const 0) (i32.const 1) (i32.const 3) (i32.const 32) (i32.const 3)))))
        (i32.const 4096))
      (func (export "call") (param i32 i32 i32 i32 i32) (result i32)
        (call $get (local.get 0) (local.get 1) (local.get 2) (local.get 3) (local.get 4))))
    "#;

fn guest_u32(store: &mut Store<HostState>, instance: &Instance, offset: usize) -> u32 {
    let memory = instance.get_memory(&mut *store, "memory").unwrap();
    let mut bytes = [0; 4];
    memory.read(&*store, offset, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

fn guest_buffer(store: &mut Store<HostState>, instance: &Instance) -> Vec<u8> {
    let ptr = guest_u32(store, instance, 0) as usize;
    let size = guest_u32(store, instance, 4) as usize;
    let memory = instance.get_memory(&mut *store, "memory").unwrap();
    let mut bytes = vec![0; size];
    memory.read(&*store, ptr, &mut bytes).unwrap();
    bytes
}

#[test]
fn get_buffer_bytes_preserves_unsigned_ranges_and_status_precedence() {
    let (mut store, instance) = instantiate_guest(GET_BUFFER_BYTES_GUEST);
    let get = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "call")
        .unwrap();
    let state = store.data_mut();
    state.vm_config = b"abcdef".to_vec();
    state.plugin_config = b"abcdef".to_vec();
    state.frame.request_body = b"abcdef".to_vec();
    state.frame.body_chunk = Bytes::from_static(b"abcdef");
    state.frame.callout_response = Some(CalloutResponse {
        body: b"abcdef".to_vec(),
        ..Default::default()
    });
    for id in [
        BUF_VM_CONFIGURATION,
        BUF_PLUGIN_CONFIGURATION,
        BUF_HTTP_REQUEST_BODY,
        BUF_HTTP_RESPONSE_BODY,
        BUF_HTTP_CALL_RESPONSE_BODY,
    ] {
        assert_eq!(get.call(&mut store, (id, 1, 2, 0, 4)).unwrap(), STATUS_OK);
        assert_eq!(guest_buffer(&mut store, &instance), b"bc");
        assert_eq!(guest_u32(&mut store, &instance, 20), 2);
        for max_size in [-1, i32::MIN, 100] {
            assert_eq!(get.call(&mut store, (id, 2, max_size, 0, 4)).unwrap(), STATUS_OK);
            assert_eq!(guest_buffer(&mut store, &instance), b"cdef");
        }
        assert_eq!(get.call(&mut store, (id, 0, -1, 0, 4)).unwrap(), STATUS_OK);
        assert_eq!(guest_buffer(&mut store, &instance), b"abcdef");
        let count = guest_u32(&mut store, &instance, 16);
        for start in [-1, i32::MIN, 6, 100] {
            assert_eq!(
                get.call(&mut store, (id, start, 0, i32::MAX, i32::MAX)).unwrap(),
                STATUS_BAD_ARGUMENT
            );
        }
        assert_eq!(
            guest_u32(&mut store, &instance, 16),
            count,
            "invalid ranges must not call the allocator"
        );
        assert_eq!(get.call(&mut store, (id, 1, 0, 0, 4)).unwrap(), STATUS_OK);
        assert!(guest_buffer(&mut store, &instance).is_empty());
        assert_eq!(guest_u32(&mut store, &instance, 16), count + 1);
    }
    let count = guest_u32(&mut store, &instance, 16);
    assert_eq!(
        get.call(&mut store, (999, -1, -1, i32::MAX, i32::MAX)).unwrap(),
        STATUS_NOT_FOUND
    );
    assert_eq!(guest_u32(&mut store, &instance, 16), count);
    assert_eq!(
        get.call(&mut store, (BUF_HTTP_REQUEST_BODY, 1, 2, i32::MAX, 4))
            .unwrap(),
        STATUS_INVALID_MEMORY_ACCESS
    );
}

#[test]
fn get_buffer_bytes_empty_sources_still_invoke_guest_allocator() {
    let (mut store, instance) = instantiate_guest(GET_BUFFER_BYTES_GUEST);
    let get = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "call")
        .unwrap();
    for id in [
        BUF_VM_CONFIGURATION,
        BUF_PLUGIN_CONFIGURATION,
        BUF_HTTP_REQUEST_BODY,
        BUF_HTTP_RESPONSE_BODY,
        BUF_HTTP_CALL_RESPONSE_BODY,
    ] {
        let count = guest_u32(&mut store, &instance, 16);
        assert_eq!(get.call(&mut store, (id, -1, -1, 0, 4)).unwrap(), STATUS_OK);
        assert!(guest_buffer(&mut store, &instance).is_empty());
        assert_eq!(guest_u32(&mut store, &instance, 16), count + 1);
        assert_eq!(guest_u32(&mut store, &instance, 20), 0);
    }
    let mode = instance
        .get_typed_func::<i32, ()>(&mut store, "allocator_mode")
        .unwrap();
    mode.call(&mut store, 1).unwrap();
    let count = guest_u32(&mut store, &instance, 16);
    assert_eq!(
        get.call(&mut store, (BUF_HTTP_CALL_RESPONSE_BODY, -1, 0, 0, 4))
            .unwrap(),
        STATUS_INTERNAL_FAILURE
    );
    assert_eq!(guest_u32(&mut store, &instance, 16), count + 1);
}

#[test]
fn get_buffer_bytes_snapshot_survives_allocator_reentry_mutation() {
    let (mut store, instance) = instantiate_guest(GET_BUFFER_BYTES_GUEST);
    store
        .data_mut()
        .begin_phase(WasmPhase::RequestBody, vec![], vec![], HashMap::new());
    store.data_mut().frame.request_body = b"abcdef".to_vec();
    store.data_mut().frame.request_body_limit = 16;
    let mode = instance
        .get_typed_func::<i32, ()>(&mut store, "allocator_mode")
        .unwrap();
    mode.call(&mut store, 2).unwrap();
    let get = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "call")
        .unwrap();
    assert_eq!(
        get.call(&mut store, (BUF_HTTP_REQUEST_BODY, 1, 3, 0, 4)).unwrap(),
        STATUS_OK
    );
    assert_eq!(guest_u32(&mut store, &instance, 24), STATUS_OK as u32);
    assert_eq!(store.data().frame.request_body, b"aXYZef");
    assert!(store.data().frame.request_body_modified);
    assert_eq!(
        guest_buffer(&mut store, &instance),
        b"bcd",
        "outer GET must return its pre-allocation range"
    );
}

const ALLOCATION_PREFLIGHT_GUEST: &str = r#"
    (module
      (import "env" "proxy_log"
        (func $log (param i32 i32 i32) (result i32)))
      (import "env" "proxy_replace_header_map_value"
        (func $replace (param i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_set_header_map_pairs"
        (func $set_pairs (param i32 i32 i32) (result i32)))
      (import "env" "proxy_send_local_response"
        (func $local (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_set_property"
        (func $set_property (param i32 i32 i32 i32) (result i32)))
      (import "env" "proxy_set_shared_data"
        (func $set_shared (param i32 i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 4)
      (global $heap (mut i32) (i32.const 196608))
      (func (export "proxy_on_memory_allocate") (param i32) (result i32)
        (local $ptr i32)
        global.get $heap
        local.set $ptr
        global.get $heap
        local.get 0
        i32.add
        global.set $heap
        local.get $ptr)
      (func (export "log") (param i32 i32) (result i32)
        (call $log (i32.const 1) (local.get 0) (local.get 1)))
      (func (export "replace") (param i32 i32 i32 i32 i32) (result i32)
        (call $replace (local.get 0) (local.get 1) (local.get 2)
                       (local.get 3) (local.get 4)))
      (func (export "set_pairs") (param i32 i32 i32) (result i32)
        (call $set_pairs (local.get 0) (local.get 1) (local.get 2)))
      (func (export "local") (param i32 i32 i32 i32 i32) (result i32)
        (call $local (local.get 0) (i32.const 0) (i32.const 0)
                     (local.get 1) (local.get 2) (local.get 3) (local.get 4)
                     (i32.const 0)))
      (func (export "set_property") (param i32 i32 i32 i32) (result i32)
        (call $set_property (local.get 0) (local.get 1)
                            (local.get 2) (local.get 3)))
      (func (export "set_shared") (param i32 i32 i32 i32 i32) (result i32)
        (call $set_shared (local.get 0) (local.get 1) (local.get 2)
                          (local.get 3) (local.get 4)))
    )
    "#;

fn instantiate_allocation_guest() -> (Store<HostState>, Instance) {
    instantiate_guest(ALLOCATION_PREFLIGHT_GUEST)
}

fn set_test_allocation_limits(store: &mut Store<HostState>, transient_bytes: usize, retained_bytes: usize) {
    store.data_mut().allocation_limits = AbiAllocationLimits {
        transient_bytes,
        retained_bytes,
        retained_ops: 16,
    };
}

fn write_guest(instance: &Instance, store: &mut Store<HostState>, offset: usize, bytes: &[u8]) {
    instance
        .get_memory(&mut *store, "memory")
        .expect("guest memory")
        .write(store, offset, bytes)
        .expect("guest write");
}

#[test]
fn allocation_profiles_enforce_exact_boundaries_and_utf8_expansion() {
    let profile = AllocationProfile::default().add_bytes(17).unwrap();
    assert_eq!(profile.checked_against(17, STATUS_INTERNAL_FAILURE), Ok(profile));
    assert_eq!(
        profile.checked_against(16, STATUS_INTERNAL_FAILURE),
        Err(STATUS_INTERNAL_FAILURE)
    );

    let invalid = [0xFF, b'x', 0xF0, 0x80, 0x80, 0x80];
    assert_eq!(lossy_utf8_output_len(&invalid), Ok(16));
    let encoded = encode_raw_map(&[(&invalid, b"v")]);
    let map = scan_serialized_map(&encoded).unwrap();
    let allocation = decoded_map_allocation(map).unwrap();
    assert_eq!(allocation.bytes, 17 + size_of::<(String, String)>());
}

#[test]
fn log_preflight_rejection_has_no_partial_state_or_accounting() {
    let (mut store, instance) = instantiate_allocation_guest();
    write_guest(&instance, &mut store, 0, b"abc");
    let log = instance.get_typed_func::<(i32, i32), i32>(&mut store, "log").unwrap();

    let allocation = 3 + size_of::<(i32, String)>();
    set_test_allocation_limits(&mut store, allocation, allocation + ABI_OP_OVERHEAD_BYTES);
    assert_eq!(log.call(&mut store, (0, 3)).unwrap(), STATUS_OK);
    let charged = store.data().frame.retained_bytes;
    assert_eq!(charged, allocation + ABI_OP_OVERHEAD_BYTES);
    assert_eq!(store.data().frame.log_lines.len(), 1);
    assert_eq!(log.call(&mut store, (0, 3)).unwrap(), STATUS_INTERNAL_FAILURE);
    assert_eq!(store.data().frame.retained_bytes, charged);
    assert_eq!(store.data().frame.retained_ops, 1);
    assert_eq!(store.data().frame.log_lines.len(), 1);

    let (mut store, instance) = instantiate_allocation_guest();
    write_guest(&instance, &mut store, 0, b"abc");
    let log = instance.get_typed_func::<(i32, i32), i32>(&mut store, "log").unwrap();
    set_test_allocation_limits(&mut store, 2, usize::MAX);
    assert_eq!(log.call(&mut store, (0, 3)).unwrap(), STATUS_INTERNAL_FAILURE);
    assert!(store.data().frame.log_lines.is_empty());
    assert_eq!(store.data().frame.retained_bytes, 0);
    assert_eq!(store.data().frame.retained_ops, 0);
}

#[test]
fn header_name_ascii_folding_preserves_bytes_and_allocation() {
    fn scalar_fold(value: &str) -> Result<String, i32> {
        let mut normalized = try_string_with_capacity(value.len())?;
        normalized.extend(value.chars().map(|character| character.to_ascii_lowercase()));
        Ok(normalized)
    }

    let ascii: String = (0u8..=127).map(char::from).collect();
    let long = "X-FIELD-é-É-π-€".repeat(128);
    for value in ["", "x-field", "X-MiXeD-Header", "éÉñÑπ€", ascii.as_str(), long.as_str()] {
        let (previous, previous_stats) =
            crate::test_allocation_counter::count_allocation_stats(|| scalar_fold(value).unwrap());
        let (actual, actual_stats) =
            crate::test_allocation_counter::count_allocation_stats(|| try_ascii_lowercase(value).unwrap());
        assert_eq!(actual, previous);
        assert_eq!(actual.len(), value.len());
        assert_eq!(actual_stats.calls, previous_stats.calls);
        assert_eq!(actual_stats.requested_bytes, previous_stats.requested_bytes);
    }
    let pairs = ["X-é", "x-é", "X-É", "x-É", "", ""]
        .into_iter()
        .map(|name| (name.to_owned(), String::new()))
        .collect::<Vec<_>>();
    assert_eq!(collect_header_removals(&pairs).unwrap(), ["X-é", "X-É", ""]);
}

#[test]
fn replace_all_profiles_structure_and_keeps_first_header_spelling() {
    let existing = vec![
        ("First".to_string(), "1".to_string()),
        ("FIRST".to_string(), "2".to_string()),
        ("Second".to_string(), "3".to_string()),
        ("second".to_string(), "4".to_string()),
    ];
    assert_eq!(collect_header_removals(&existing).unwrap(), vec!["First", "Second"]);

    let encoded = encode_raw_map(&[(b"new", b"value")]);
    let map_profile = scan_serialized_map(&encoded).unwrap();
    let base = decoded_map_allocation(map_profile).unwrap().repeated(2).unwrap();
    let (removal_retained, removal_transient) = header_removal_profiles(&existing).unwrap();
    let pending_growth = vector_growth_allocation::<HeaderOp>(0, 0, 1).unwrap();
    let retained = base
        .add_bytes(removal_retained.bytes)
        .and_then(|profile| profile.add_bytes(pending_growth.bytes))
        .unwrap();
    let transient = base
        .add_bytes(removal_transient.bytes)
        .and_then(|profile| profile.add_bytes(pending_growth.bytes))
        .unwrap();

    let (mut store, instance) = instantiate_allocation_guest();
    store.data_mut().frame.request_headers = existing.clone();
    write_guest(&instance, &mut store, 0, &encoded);
    set_test_allocation_limits(&mut store, transient.bytes, retained.bytes + ABI_OP_OVERHEAD_BYTES);
    let set_pairs = instance
        .get_typed_func::<(i32, i32, i32), i32>(&mut store, "set_pairs")
        .unwrap();
    assert_eq!(
        set_pairs
            .call(&mut store, (MAP_HTTP_REQUEST_HEADERS, 0, encoded.len() as i32))
            .unwrap(),
        STATUS_OK
    );
    assert_eq!(
        store.data().frame.request_headers,
        vec![("new".to_string(), "value".to_string())]
    );
    assert!(matches!(
        store.data().frame.pending_ops.as_slice(),
        [HeaderOp::ReplaceAll { remove, .. }] if remove == &["First", "Second"]
    ));

    let (mut store, instance) = instantiate_allocation_guest();
    store.data_mut().frame.request_headers = existing.clone();
    write_guest(&instance, &mut store, 0, &encoded);
    set_test_allocation_limits(&mut store, transient.bytes - 1, usize::MAX);
    let set_pairs = instance
        .get_typed_func::<(i32, i32, i32), i32>(&mut store, "set_pairs")
        .unwrap();
    assert_eq!(
        set_pairs
            .call(&mut store, (MAP_HTTP_REQUEST_HEADERS, 0, encoded.len() as i32))
            .unwrap(),
        STATUS_INTERNAL_FAILURE
    );
    assert_eq!(store.data().frame.request_headers, existing);
    assert!(store.data().frame.pending_ops.is_empty());
    assert_eq!(store.data().frame.retained_bytes, 0);
    assert_eq!(store.data().frame.retained_ops, 0);

    let (mut store, instance) = instantiate_allocation_guest();
    store.data_mut().frame.request_headers = vec![("kept".to_string(), "value".to_string())];
    write_guest(&instance, &mut store, 0, &encoded);
    let set_pairs = instance
        .get_typed_func::<(i32, i32, i32), i32>(&mut store, "set_pairs")
        .unwrap();
    assert_eq!(
        set_pairs.call(&mut store, (99, 0, encoded.len() as i32)).unwrap(),
        STATUS_BAD_ARGUMENT
    );
    let malformed = [0xFF, 0xFF, 0xFF, 0xFF];
    write_guest(&instance, &mut store, 64, &malformed);
    assert_eq!(
        set_pairs
            .call(&mut store, (MAP_HTTP_REQUEST_HEADERS, 64, malformed.len() as i32),)
            .unwrap(),
        STATUS_BAD_ARGUMENT
    );
    assert_eq!(
        store.data().frame.request_headers,
        vec![("kept".to_string(), "value".to_string())]
    );
    assert!(store.data().frame.pending_ops.is_empty());
    assert_eq!(store.data().frame.retained_bytes, 0);
    assert_eq!(store.data().frame.retained_ops, 0);
}

#[test]
fn high_segment_property_uses_exact_single_string_profile() {
    let (mut store, instance) = instantiate_allocation_guest();
    let mut path = Vec::new();
    for index in 0..2048 {
        if index != 0 {
            path.push(0);
        }
        path.push(if index == 1024 { 0xFF } else { b'a' });
    }
    let decoded_len = path.len() + 2;
    let value = b"v";
    write_guest(&instance, &mut store, 0, &path);
    write_guest(&instance, &mut store, 8192, value);
    let structural = conservative_hash_table_allocation::<(String, String)>(1).unwrap().bytes;
    let allocation = decoded_len + value.len() + structural;
    set_test_allocation_limits(&mut store, allocation, allocation + ABI_OP_OVERHEAD_BYTES);
    let set_property = instance
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "set_property")
        .unwrap();
    assert_eq!(
        set_property
            .call(&mut store, (0, path.len() as i32, 8192, value.len() as i32))
            .unwrap(),
        STATUS_OK
    );
    assert_eq!(store.data().frame.properties.len(), 1);
    let key = store.data().frame.properties.keys().next().unwrap();
    assert_eq!(key.len(), decoded_len);
    assert!(key.contains('\u{FFFD}'));

    let (mut store, instance) = instantiate_allocation_guest();
    write_guest(&instance, &mut store, 0, &path);
    write_guest(&instance, &mut store, 8192, value);
    set_test_allocation_limits(&mut store, allocation - 1, usize::MAX);
    let set_property = instance
        .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "set_property")
        .unwrap();
    assert_eq!(
        set_property
            .call(&mut store, (0, path.len() as i32, 8192, value.len() as i32))
            .unwrap(),
        STATUS_INTERNAL_FAILURE
    );
    assert!(store.data().frame.properties.is_empty());
    assert_eq!(store.data().frame.retained_bytes, 0);
    assert_eq!(store.data().frame.retained_ops, 0);
}

#[test]
fn local_response_preflight_is_atomic_and_keeps_compatibility_precedence() {
    let body = b"body";
    let headers = encode_raw_map(&[(b"x-test", b"yes")]);
    let header_allocation = decoded_map_allocation(scan_serialized_map(&headers).unwrap())
        .unwrap()
        .bytes;
    let allocation = body.len() + header_allocation;
    let (mut store, instance) = instantiate_allocation_guest();
    write_guest(&instance, &mut store, 0, body);
    write_guest(&instance, &mut store, 64, &headers);
    set_test_allocation_limits(&mut store, allocation, usize::MAX);
    let local = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "local")
        .unwrap();
    assert_eq!(
        local
            .call(&mut store, (418, 0, body.len() as i32, 64, headers.len() as i32))
            .unwrap(),
        STATUS_OK
    );
    let accepted = store.data().frame.local_response.clone().unwrap();
    assert_eq!(accepted.body, body);
    assert_eq!(accepted.headers, vec![("x-test".to_string(), "yes".to_string())]);

    set_test_allocation_limits(&mut store, allocation - 1, usize::MAX);
    assert_eq!(
        local
            .call(&mut store, (500, 0, body.len() as i32, 64, headers.len() as i32))
            .unwrap(),
        STATUS_INTERNAL_FAILURE
    );
    assert_eq!(
        store.data().frame.local_response.as_ref().unwrap().status,
        accepted.status
    );
    assert_eq!(store.data().frame.local_response.as_ref().unwrap().body, accepted.body);
    assert_eq!(
        store.data().frame.local_response.as_ref().unwrap().headers,
        accepted.headers
    );

    let malformed = [0xFF, 0xFF, 0xFF, 0xFF];
    write_guest(&instance, &mut store, 128, &malformed);
    let existing_allocation = local_response_allocation(store.data().frame.local_response.as_ref().unwrap())
        .unwrap()
        .bytes;
    set_test_allocation_limits(&mut store, existing_allocation, usize::MAX);
    assert_eq!(
        local
            .call(&mut store, (204, i32::MAX, 0, 128, malformed.len() as i32))
            .unwrap(),
        STATUS_OK,
        "nonpositive body ignores its pointer and malformed headers become empty"
    );
    assert!(store.data().frame.local_response.as_ref().unwrap().body.is_empty());
    assert!(store.data().frame.local_response.as_ref().unwrap().headers.is_empty());
    assert_eq!(
        local.call(&mut store, (204, 300_000, 1, 300_000, 1)).unwrap(),
        STATUS_INVALID_MEMORY_ACCESS,
        "positive body validation remains first"
    );
}

fn local_response_with_spare_capacity() -> LocalResponse {
    let mut body = Vec::with_capacity(32);
    body.extend_from_slice(b"old");
    let mut name = String::with_capacity(16);
    name.push_str("x-old");
    let mut value = String::with_capacity(24);
    value.push_str("present");
    let mut headers = Vec::with_capacity(4);
    headers.push((name, value));
    LocalResponse {
        status: 409,
        body,
        headers,
    }
}

#[test]
fn local_response_replacement_profiles_old_and_candidate_peak() {
    let candidate_body = b"new-body";
    let candidate_headers = encode_raw_map(&[(b"x-new", b"accepted")]);
    let candidate_allocation = AllocationProfile::default()
        .add_bytes(candidate_body.len())
        .and_then(|profile| {
            profile.add_bytes(
                decoded_map_allocation(scan_serialized_map(&candidate_headers).unwrap())
                    .unwrap()
                    .bytes,
            )
        })
        .unwrap();

    let (mut store, instance) = instantiate_allocation_guest();
    store.data_mut().frame.local_response = Some(local_response_with_spare_capacity());
    let old_allocation = local_response_allocation(store.data().frame.local_response.as_ref().unwrap()).unwrap();
    let exact_peak = old_allocation.add_bytes(candidate_allocation.bytes).unwrap().bytes;
    write_guest(&instance, &mut store, 0, candidate_body);
    write_guest(&instance, &mut store, 64, &candidate_headers);
    set_test_allocation_limits(&mut store, exact_peak, usize::MAX);
    let local = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "local")
        .unwrap();
    assert_eq!(
        local
            .call(
                &mut store,
                (201, 0, candidate_body.len() as i32, 64, candidate_headers.len() as i32,),
            )
            .unwrap(),
        STATUS_OK
    );
    assert_eq!(store.data().frame.local_response.as_ref().unwrap().body, candidate_body);

    let (mut store, instance) = instantiate_allocation_guest();
    store.data_mut().frame.local_response = Some(local_response_with_spare_capacity());
    let preserved_allocation = local_response_allocation(store.data().frame.local_response.as_ref().unwrap()).unwrap();
    let rejected_peak = preserved_allocation
        .add_bytes(candidate_allocation.bytes)
        .unwrap()
        .bytes;
    store.data_mut().frame.retained_bytes = 123;
    store.data_mut().frame.retained_ops = 2;
    write_guest(&instance, &mut store, 0, candidate_body);
    write_guest(&instance, &mut store, 64, &candidate_headers);
    set_test_allocation_limits(&mut store, rejected_peak - 1, usize::MAX);
    let local = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "local")
        .unwrap();
    assert_eq!(
        local
            .call(
                &mut store,
                (201, 0, candidate_body.len() as i32, 64, candidate_headers.len() as i32,),
            )
            .unwrap(),
        STATUS_INTERNAL_FAILURE
    );
    let preserved = store.data().frame.local_response.as_ref().unwrap();
    assert_eq!(preserved.status, 409);
    assert_eq!(preserved.body, b"old");
    assert_eq!(preserved.body.capacity(), 32);
    assert_eq!(preserved.headers, vec![("x-old".to_string(), "present".to_string())]);
    assert_eq!(preserved.headers.capacity(), 4);
    assert_eq!(preserved.headers[0].0.capacity(), 16);
    assert_eq!(preserved.headers[0].1.capacity(), 24);
    assert_eq!(store.data().frame.retained_bytes, 123);
    assert_eq!(store.data().frame.retained_ops, 2);
}

#[test]
fn shared_data_preflight_preserves_ranges_empty_writes_and_cas() {
    let (mut store, instance) = instantiate_allocation_guest();
    write_guest(&instance, &mut store, 0, b"k");
    write_guest(&instance, &mut store, 16, b"xy");
    let set_shared = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "set_shared")
        .unwrap();

    assert_eq!(set_shared.call(&mut store, (0, 1, 16, 0, 0)).unwrap(), STATUS_OK);
    assert_eq!(store.data().shared_data.get("k").unwrap().0, Vec::<u8>::new());
    assert_eq!(
        set_shared.call(&mut store, (0, 1, 250_000, 65_537, 0)).unwrap(),
        STATUS_INVALID_MEMORY_ACCESS,
        "an invalid range precedes the 64 KiB value cap"
    );

    let shared = store.data().shared_data.clone();
    shared.set("k", Some(b"x".to_vec()), 0).unwrap();
    let target_bytes = ENTRY_OVERHEAD_BYTES + 2;
    let filler_len = MAX_SHARED_TOTAL_BYTES - target_bytes - ENTRY_OVERHEAD_BYTES;
    let filler = "f".repeat(filler_len);
    shared.set(&filler, Some(Vec::new()), 0).unwrap();
    assert_eq!(shared.total_bytes(), MAX_SHARED_TOTAL_BYTES);
    assert_eq!(
        set_shared.call(&mut store, (0, 1, 16, 2, 999)).unwrap(),
        STATUS_CAS_MISMATCH,
        "CAS mismatch remains authoritative over the total-byte cap"
    );
    assert_eq!(shared.get("k").unwrap().0, b"x");

    let (mut store, instance) = instantiate_allocation_guest();
    write_guest(&instance, &mut store, 0, b"k");
    write_guest(&instance, &mut store, 16, b"v");
    set_test_allocation_limits(&mut store, 130, usize::MAX);
    let set_shared = instance
        .get_typed_func::<(i32, i32, i32, i32, i32), i32>(&mut store, "set_shared")
        .unwrap();
    assert_eq!(
        set_shared.call(&mut store, (0, 1, 16, 1, 0)).unwrap(),
        STATUS_BAD_ARGUMENT
    );
    assert!(store.data().shared_data.get("k").is_none());
    assert_eq!(store.data().frame.retained_bytes, 0);
    assert_eq!(store.data().frame.retained_ops, 0);
}

#[test]
fn callout_tokens_survive_frames_and_exhaust_without_reuse() {
    let (mut store, instance) = instantiate_callout_guest();
    let dispatch = instance.get_typed_func::<i32, i32>(&mut store, "dispatch").unwrap();
    let memory = instance.get_memory(&mut store, "memory").unwrap();
    for expected in [1u32, 2] {
        store
            .data_mut()
            .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
        store
            .data_mut()
            .arm_dispatch(Arc::from(vec!["http://example.com:80".into()]), 1000);
        assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_OK);
        let mut bytes = [0; 4];
        memory.read(&store, 128, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), expected);
    }
    store.data_mut().last_callout_token = u32::MAX;
    store
        .data_mut()
        .begin_phase(WasmPhase::RequestHeaders, vec![], vec![], HashMap::new());
    store
        .data_mut()
        .arm_dispatch(Arc::from(vec!["http://example.com:80".into()]), 1000);
    assert_eq!(dispatch.call(&mut store, 128).unwrap(), STATUS_INTERNAL_FAILURE);
    assert!(store.data().frame.pending_callouts.is_empty());
    assert_eq!(store.data().last_callout_token, u32::MAX);
}

#[test]
fn configured_host_allocation_limits_tighten_the_existing_profiles() {
    crate::test_subprocess::assert_in_subprocess(
        "plugins::http::wasm::host_abi::tests",
        "configured_host_allocation_limits_tighten_the_existing_profiles",
        "EDGION_WASM_HOST_LIMITS_CHILD",
        "bounded",
        || {
            super::super::admission::initialize(super::super::admission::WasmLimits {
                max_host_call_bytes: 256,
                max_host_abi_bytes: 128,
                max_host_abi_ops: 1,
                ..Default::default()
            })
            .unwrap();
            let mut host = HostState::default();
            assert_eq!(host.allocation_limits.transient_bytes, 256);
            assert_eq!(host.allocation_limits.retained_bytes, 128);
            assert_eq!(host.allocation_limits.retained_ops, 1);
            assert!(host
                .check_transient_allocation(AllocationProfile { bytes: 256 })
                .is_ok());
            assert!(host
                .check_transient_allocation(AllocationProfile { bytes: 257 })
                .is_err());
            assert!(host.charge_abi(65).is_err());
            assert_eq!(host.frame.retained_bytes, 0);
            assert_eq!(host.frame.retained_ops, 0);
            assert!(host.charge_abi(64).is_ok());
            assert!(host.charge_abi(0).is_err());
            assert_eq!(host.frame.retained_bytes, 128);
            host.release_phase_payloads();
            assert!(host.charge_abi(0).is_ok());
        },
    );
}

#[test]
fn guest_input_profiles_preserve_memory_error_precedence() {
    for (memory, has_memory) in [
        ("(memory (export \"memory\") 4)", true),
        ("(memory 4)", false),
        ("(memory 4) (global (export \"memory\") i32 (i32.const 0))", false),
    ] {
        let wat = ALLOCATION_PREFLIGHT_GUEST.replace("(memory (export \"memory\") 4)", memory);
        let (mut store, instance) = instantiate_guest(&wat);
        let log = instance.get_typed_func::<(i32, i32), i32>(&mut store, "log").unwrap();
        let set_pairs = instance
            .get_typed_func::<(i32, i32, i32), i32>(&mut store, "set_pairs")
            .unwrap();
        let set_property = instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "set_property")
            .unwrap();
        let mut cases = vec![
            (-1, 0, STATUS_BAD_ARGUMENT),
            (0, -1, STATUS_BAD_ARGUMENT),
            (i32::MAX, 1, STATUS_INVALID_MEMORY_ACCESS),
            (0, i32::MAX, STATUS_INVALID_MEMORY_ACCESS),
        ];
        if !has_memory {
            cases.push((0, 0, STATUS_INVALID_MEMORY_ACCESS));
        }
        for (ptr, len, expected) in cases {
            assert_eq!(log.call(&mut store, (ptr, len)).unwrap(), expected);
            assert_eq!(
                set_pairs
                    .call(&mut store, (MAP_HTTP_REQUEST_HEADERS, ptr, len))
                    .unwrap(),
                expected
            );
            assert_eq!(set_property.call(&mut store, (ptr, len, 0, 0)).unwrap(), expected);
            assert!(store.data().frame.log_lines.is_empty());
            assert!(store.data().frame.pending_ops.is_empty());
            assert!(store.data().frame.properties.is_empty());
            assert_eq!(store.data().frame.retained_bytes, 0);
            assert_eq!(store.data().frame.retained_ops, 0);
        }
    }
}
