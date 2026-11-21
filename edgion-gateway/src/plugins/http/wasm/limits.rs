//! Reviewed proxy-wasm safety envelopes shared by the ABI host and the
//! restart-only admission limits. Kept free of Wasmtime so `admission` and the
//! gateway configuration compile without the `wasm` feature.

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

/// Per-phase cap on guest-controlled header writes.
///
/// Every queued header write is later applied to the live request/response
/// header map. Pingora's `insert_header` uses the panicking
/// `http::HeaderMap::insert`, which panics once the map cannot grow: at
/// 24,576 distinct names, or from 6,554 names when crafted hash collisions put
/// the map in its displacement-danger state. The retention budgets above admit
/// tens of thousands of small pairs, so they cannot bound names on their own.
///
/// `proxy_replace_header_map_value` and `proxy_add_header_map_value` count as
/// one write each; `proxy_set_header_map_pairs` counts its pair count;
/// removals do not count. `proxy_send_local_response` replaces (rather than
/// accumulates) its header list, so it is bounded per call by the same value.
/// 1024 leaves room for a guest that rewrites a whole header map and matches
/// the callout response header bound (`validate_response_headers`). Only the
/// request-headers, request-body and response-headers phases apply writes, so
/// one plugin adds at most 3,072 request and about 4,096 response names, which
/// keeps a single plugin below 6,554 together with downstream request headers
/// or an HTTP/1 upstream response.
pub const MAX_HEADER_WRITES_PER_PHASE: usize = 1024;

/// Per-call cap on the header pairs a guest passes to `proxy_http_call`,
/// pseudo-headers included.
///
/// The callout request is built with reqwest, whose `RequestBuilder::header`
/// and default-header merge in `Client::execute` use the panicking
/// `http::HeaderMap::append` / `entry`. The map cannot grow past 24,576
/// distinct names, or past 6,554 names once crafted hash collisions put it in
/// its displacement-danger state. The retention budgets above admit tens of
/// thousands of small pairs, so they cannot bound names on their own. 1024
/// matches the callout response header bound (`validate_response_headers`).
pub const MAX_CALLOUT_REQUEST_HEADER_PAIRS: usize = 1024;

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
pub const MAX_GUEST_TABLE_ELEMENTS: usize = 20_000;
