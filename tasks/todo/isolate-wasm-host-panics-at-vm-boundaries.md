# Isolate Wasm Host Panics at VM Boundaries

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-02 |
| Status | completed |
| Type | security |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Prevent Rust host-function panics resumed by Wasmtime from escaping any Gateway Wasm typed-call boundary or returning an unpoisoned VM to the pool.

## Requirements

- Inventory every root-context, request/response, callout-response, tick, close, and initialization typed-call boundary in `vm_manager.rs`.
- Wrap each boundary with the canonical `edgion_common::startup::catch_redacted_panic` pattern.
- Convert a caught panic to one fixed generic internal error without exposing the payload in logs, plugin responses, or access-log diagnostics.
- Poison/discard the VM before any lease can return to the pool.
- Preserve ordinary guest-trap, epoch-timeout, host-`Err`, cancellation, and hot-reload behavior.
- Do not modify existing integration-test files.

## Acceptance criteria

- No Rust host-import panic can unwind into the Pingora worker loop from a Wasm typed call.
- Every caught panic deterministically prevents VM reuse and returns only a generic error.
- A crate-local panicking-host regression distinguishes Rust panic from guest `unreachable`, proves discard, and proves a later clean VM recovers.
- Wasm tests, workspace format/check/Clippy, Gateway build, no-tracing, English-only, observability, source-policy, and frozen-path guards pass.

## Completion evidence

Historical verification of `edgion-gateway/src/plugins/http/wasm/vm_manager.rs` covered the complete boundary. Independent review approved all initialization, request/response, callout, tick, done, and delete paths. At that revision, 59 VM-manager tests, 206 Gateway Wasm tests, 23 `edgion-wasm` tests, workspace format/check/Clippy, Gateway build, and repository guards passed. These are historical results, not fresh verification of subsequent source changes.
