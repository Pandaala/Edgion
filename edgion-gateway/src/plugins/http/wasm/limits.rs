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
