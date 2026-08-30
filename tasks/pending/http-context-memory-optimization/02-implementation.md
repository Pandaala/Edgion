# Implementation and Verification

## Baseline measurements

| Measurement | ARM64 baseline | Current | Final |
|-------------|---------------:|--------:|------:|
| `size_of::<EdgionHttpContext>()` | 2,672 bytes | 2,672 bytes | pending |
| `Option<Box<WafRequestContext>>` | 8 bytes | 8 bytes | pending |
| Default Context allocation calls | 1 | 0 | pending |
| Default Context requested allocation bytes | 192 | 0 | pending |

The baseline allocation was the `Vec::with_capacity(3)` reservation for
`stage_logs`. Stage 1 removed the eager reservation, making the default Context
constructor allocation-free. Allocator bookkeeping is not included in
requested bytes.

## Per-stage verification

- Apply `cargo fmt --all`.
- Run targeted unit tests for the changed state boundary.
- Run `cargo check -p edgion-gateway --lib`.
- Run relevant Clippy checks and record pre-existing warnings separately.
- Run `git diff --check`.
- Ask an independent subagent to review correctness, lifecycle, allocation, and
  hot-path effects.
- Resolve every material finding before committing.

## Final verification

- `cargo fmt --all -- --check`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets`
- `python3 cicd/checks/validate_agent_docs.py`
- `python3 cicd/checks/check_ssa_force.py`
- `python3 cicd/checks/check_metrics_inventory.py`
- `python3 cicd/checks/check_pingora_source_policy.py`
- Targeted Gateway unit tests for every refactored lifecycle.
- Internal context layout/allocation benchmark scenarios.

`cargo test --all` is outside the default quick loop and is run only if
explicitly requested.

## Acceptance gates

- No behavior, serialization, retry, or hot-reload regression.
- No new allocation on ordinary HTTP requests.
- Default `EdgionHttpContext::new()` becomes allocation-free.
- Optional scenario state performs no more than one state allocation when
  first activated, excluding the scenario's inherently owned payloads.
- Request and response per-chunk paths do not create `PluginLog`, `String`, or
  an input-length-dependent buffer.
- WAF is absent from generic `stage_logs` and appears at most once as a bounded
  top-level access-log block.
