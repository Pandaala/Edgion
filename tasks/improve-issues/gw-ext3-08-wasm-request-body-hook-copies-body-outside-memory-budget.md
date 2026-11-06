# gw-ext3-08: Wasm request-body hook copies the whole body into an unbudgeted `Vec`, bypassing BodyStorage memory admission

- **Category**: design-error
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/wasm/plugin.rs:319-435` (`run_request_body_phase`; `read_request_body(Some(cap))` at line 330, `state.frame.request_body = body.to_vec()` at line 406), `edgion-gateway/src/plugins/http/wasm/host_abi.rs:300` (`CallbackFrame::request_body: Vec<u8>`), `host_abi.rs:874` (`BUF_HTTP_REQUEST_BODY` reads borrow that Vec), `host_abi.rs:946-960` (`replace_request_body_buffer` splices in place), `edgion-gateway/src/request_body/storage.rs:667-686,2285-2299` (`read_all` / `read_disk_body` materialize under a `MemoryBuffer` lease)

## Summary
Request-body capture is designed around `BodyStorage`: bytes are admitted against a process memory
budget, spill to disk, and report `CapacityExceeded` (503) under pressure. The Wasm buffer-mode hook
first materializes the body through that budget (`read_all`, leased `MemoryBuffer`), and then copies
it once more with `body.to_vec()` into the guest's `HostState` so that `proxy_set_buffer_bytes` can
`splice` it. That second full-size copy is an ordinary heap allocation outside every budget: not
BodyStorage's lease, not the Wasm guest-memory budget, and not the host-ABI retained/transient
budgets. It lives for the whole guest hook and until publication.

## Current behavior (evidence)
- `run_request_body_phase` reads `cap = request_body_config.max_body_size_bytes` (default 32 MiB,
  operator-raisable) and calls `session.read_request_body(Some(cap))`, which returns budgeted
  `Bytes` (`storage.rs:667-686`; disk-backed bodies are read into a leased `MemoryBuffer`,
  `storage.rs:2285-2299`).
- It then executes `state.frame.request_body = body.to_vec();` (`plugin.rs:406`) while `body` is
  still alive, so peak usage is two full copies, one unaccounted.
- `proxy_get_buffer_bytes(HttpRequestBody)` snapshots the requested range into another owned `Vec`
  before guest allocation (`BufferSource::snapshot`, bounded only by `max_host_call_bytes`, 128 MiB),
  and the guest holds a third copy in linear memory (guest budget). Only the latter is admitted.
- Concurrency multiplies the unaccounted part: each in-flight Wasm request that reaches the body hook
  holds one, up to `max_instances` (default 128) / `maxActiveRequests`. With the default 32 MiB cap
  that is up to 4 GiB of heap invisible to the BodyStorage pressure signal; with spill-to-disk
  enabled the design intent (bounded memory for large bodies) is defeated for exactly the plugin
  most likely to see large bodies.
- The BodyStorage spill/budget guarantees are an architectural invariant in `AGENTS.md`
  ("HTTP body handling invariants": request plugins that require the complete body share the
  request-scoped `BodyStorage`; it may spill from memory to disk and is bounded").

## Industry comparison
- Envoy's Wasm `getBufferBytes` reads directly from the filter manager's buffered body
  (`Buffer::Instance` slices) and `setBufferBytes` edits that buffer in place; the only additional
  copy is into guest memory, and buffering is bounded by the connection-manager buffer limit and the
  overload manager.

## Impact
Large uploads routed through a Wasm body hook can push the gateway past its memory limit without
ever triggering BodyStorage's 503 admission, turning a designed graceful-degradation path into OOM
kills under load.

## Suggested direction
Keep the staged body inside the budget:
- Serve `get_buffer_bytes(HttpRequestBody)` ranges from the existing `Bytes` (or a snapshot reader)
  instead of a private `Vec`, and represent guest edits as a small list of range replacements that
  are applied once into a leased `MemoryBuffer` at publication (or via the existing BodyStorage
  mutation API: `remove`/`append`/`prepend`/replace segments).
- If a contiguous staging buffer is kept, allocate it with `MemoryBuffer::new(len, lease)` so it is
  charged to the same budget and refused with `CapacityExceeded` -> 503 like every other body user.

## Notes
- Checked `skills/04-review/wasm/regression-guards/wasm-whole-request-body-callback-contract.md`
  (callback count/EOS contract only) and `buffer-range-snapshot-before-guest-allocation.md`
  (justifies the per-call range snapshot, not the whole-body staging copy). No page accepts an
  unbudgeted staging copy.

## Verification (independent)

Verdict: CONFIRMED, DOWNGRADED P2 -> P3.
- `run_request_body_phase` reads the budgeted body (`wasm/plugin.rs:330`) and then does `state.frame.request_body = body.to_vec()` (`:406`) while `body` remains alive, so one extra full copy is unaccounted.
- Downgrade reason: the unaccounted copy exists only while the budgeted `read_request_body` result is held, so it is bounded by the same BodyStorage memory budget (worst case roughly 2x that budget), not unbounded; the "4 GiB invisible" figure assumes the budget admits 128 concurrent 32 MiB reads. Still an accounting violation of the body-handling invariant worth fixing (serve ranges from the `Bytes`, or lease the staging buffer).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `wasm/plugin.rs` is unchanged; `state.frame.request_body = body.to_vec()` is still at `plugin.rs:406`.
- `wasm/host_abi.rs` and `request_body/storage.rs` are unchanged; only `request_body/budget.rs:84` changed (`try_update`, same semantics).
- `wasm-whole-request-body-callback-contract.md` is unchanged.
