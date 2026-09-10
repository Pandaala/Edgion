---
name: preload-claim-before-owned-inputs
description: Review Wasm response misses for unnecessary owned inputs before the canonical preload claim.
knowledge_type: regression-guards
---

# Claim Wasm preload before copying task inputs

## Conclusion and scope

Wasm response-header and response-body misses borrow `WasmPlugin.cache_key`
through `WasmVmManager::spawn_preload` and `prepare_preload`. Only a successful
`LoadSlot::try_claim_preload` creates the owned key and configuration required
by the background task. The plugin retains its construction-time runtime key.

## Rationale and evidence

Finding `avoid-wasm-response-inflight-cache-key-allocation` (campaign source
`D0098`) identified a real, low-severity performance defect: response misses
cloned the key before checking whether preload was already claimed. Each chunk
during a cold load or negative-cache window could allocate a discarded String.

The correction is in `edgion-gateway/src/plugins/http/wasm/plugin.rs` (build and
both synchronous response callbacks) and `vm_manager.rs` (`prepare_preload`).
The engine/runtime checks, published-module check, retry deadline, and atomic
preload claim precede creation of task-owned inputs. `PreloadClaimGuard` still
releases the claim on drop, including task cancellation.

This follows the hot-path performance rules in
[the design index](../../../00-design-rules.md#u-gateway-hot-path-performance--bounded-memory).
The existing [runtime cache identity guard](wasm-module-cache-key-runtime-config.md)
still applies: borrowing changes ownership timing, not source/config identity,
registration, eviction, or runtime-config isolation. It did not previously
establish that unsuccessful preload attempts avoided key copies.

## Alternatives and limits

No extra lock, cache, task registry, metric, or response buffering is introduced.
Response fail-open/fail-close handling, body trap behavior, streaming, and
request-owned diagnostics remain unchanged. Initial slot creation still needs
an owned map key; a successful task still owns its inputs. This correction does
not claim that cold slot creation or the entire Wasm body callback allocates zero.
There is no measured throughput improvement or allocation-count benchmark.

## Verification and re-evaluation

`borrowed_preload_inputs_survive_concurrent_misses_and_retry` retains the first
claim while 64 concurrent callers attempt preparation, verifies they are rejected,
and checks caller-input independence, slot identity, claim release, negative-cache
suppression, and retry. It uses no network or timing sleeps. Allocation placement
is established by the borrowed signatures and the copy after the claim; the test
checks lifecycle behavior rather than counting allocator calls.

Current verification (2026-09-10):

- `cargo test -p edgion-gateway --lib preload`: all four tests passed, including
  the new deterministic test, cancellation release, unavailable engine, and
  the existing single-flight/negative-cache test.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed. Clippy reported warnings
  outside the changed code; this was not a warning-free workspace run.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py` under `cicd/checks/`: passed.
- Independent read-only review: LGTM. Existing integration tests were not
  modified or run; no full-workspace test run was performed.

Re-evaluate if preload gains another entry point, owned inputs move before the
claim, or cache identity, slot creation, retry, or cancellation semantics change.

## Resolution references

- `avoid-wasm-response-inflight-cache-key-allocation` / `D0098`: fixed
  2026-09-10; unnecessary key copying on unsuccessful preload attempts removed.
