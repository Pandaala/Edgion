# Investigate the plugin execution state layout baseline failure

Status: deferred for user decision. Priority: P2. Type: verification investigation.
Baseline: `25e0fefda`. Host: `aarch64-apple-darwin`, Rust 1.96.1.

## Evidence

The existing test
`ctx::plugin_request_state_tests::plugin_state_layout_keeps_hot_and_sparse_costs_separate`
fails at its first assertion in
`edgion-gateway/tests/unit/ctx_test/plugin_request_state_tests.rs:6`:
PluginExecutionState is 184 bytes, while the assertion expects 176 bytes.
The subsequent PluginAuxState and optional-box size assertions are not reached.

Reproduction on unmodified HEAD:

```sh
cargo test -p edgion-gateway --lib plugin_state_layout_keeps_hot_and_sparse_costs_separate -- --test-threads=1
```

The command exits 101 with 0 passed and 1 failed. It reproduces the same
184-versus-176 failure seen during runtime-projection verification. All four
implementation/test files of that optimization were temporarily restored to
their exact HEAD contents for the baseline run, then restored to the reviewed
working diff. No layout fields, test assertions or dependencies were changed.
Local evidence: `/tmp/edgion-scope-runtime-layout-baseline.log`.

## Decision and acceptance

Determine whether the size change represents an intended layout evolution,
a regression, or a target/toolchain-sensitive assumption before updating the
assertion or restructuring state. Inspect field histories and actual layout;
do not simply replace the expected constant to turn the test green.
Preserve inline hot-state ownership, lazy sparse state and allocation checks.
Re-run the full plugin_request_state_tests filter after the selected resolution,
including the later size assertions, and state the verified target/toolchain.

This task records a baseline failure, not acceptance of additional request
memory or evidence of a runtime-projection regression. The assertion remains
enabled and unchanged; targeted optimization verification reports its exclusion
explicitly. No full workspace test-suite result is claimed.
