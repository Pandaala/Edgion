---
name: wasm-whole-request-body-callback-contract
description: Use when reviewing Wasm request-body integration coverage; defines the complete single terminal callback contract.
status: fixed
finding_id: BODY-013
closed: 2026-07-27
knowledge_type: regression-guards
---

# Wasm Request Bodies Use One Complete Terminal Callback — FIXED

## Conclusion

The Wasm request-body integration test must verify body length, callback count,
and terminal state together. Summing observed bytes alone is not sufficient to
protect the whole-body callback contract.

## Core Rationale

**1. Request-body delivery is whole-body**

After bounded request capture completes, the Wasm guest receives the current
complete body version in one `proxy_on_request_body` callback with
`end_of_stream = true`. This differs from the streaming response-body callback,
which may observe multiple chunks.

**2. Three counters distinguish contract violations**

For the eleven-byte `hello world` request, the before/after counter delta must
equal `(bytes, calls, eos) == (11, 1, 1)`. A chunked delivery can preserve the
byte total but increases `calls`; a missing terminal flag leaves `eos` at zero.
The reference guest counts zero-length callbacks as well, so an extra empty
terminal notification cannot hide behind the correct byte total.

**3. Delta assertions preserve rerun safety**

The counters are cumulative shared Wasm state. Checked before/after deltas
isolate the current request and remain valid across repeated runs and runtime
reloads without introducing a test-only reset path. Counter-read probes are
excluded by inspecting their marker header in the body-phase context; a
per-context flag is insufficient because headers and body phases use distinct
guest contexts.

## Fix Suggestions Not Accepted

- Assert only total bytes — chunked or non-terminal delivery could pass.
- Reuse response-body observation semantics — the main upstream response is
  streaming, while eligible request bodies are captured before this callback.
- Reset shared counters before the test — per-request deltas prove the contract
  without mutating shared state.

## Re-evaluation Triggers

Re-open this decision only if:

- The Wasm request-body ABI intentionally becomes chunk-oriented.
- The runtime exposes multiple request-body callbacks with a different explicit
  completion contract.
- The reference guest replaces shared counters with request-scoped observations.

## Reference Cases

- `BODY-013`
- `edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs`
- `edgion-tests/wasm/header-demo/src/lib.rs`
- `skills/04-review/wasm/regression-guards/wasm-body-counter-delta-rerun-safe.md`

## Resolution references

- `BODY-013` — 2026-07-27.
