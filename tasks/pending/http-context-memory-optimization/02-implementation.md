# Implementation and Verification

## Baseline measurements

| Measurement | ARM64 baseline | Current | Final |
|-------------|---------------:|--------:|------:|
| `size_of::<EdgionHttpContext>()` | 2,672 bytes | 1,592 bytes | pending |
| `Option<Box<WafRequestContext>>` | 8 bytes | 8 bytes | pending |
| Default Context allocation calls | 1 | 0 | pending |
| Default Context requested allocation bytes | 192 | 0 | pending |

The baseline allocation was the `Vec::with_capacity(3)` reservation for
`stage_logs`. Stage 1 removed the eager reservation, making the default Context
constructor allocation-free. Allocator bookkeeping is not included in
requested bytes.

Stage 2a replaced the 176-byte per-request `GatewayInfo` clone with an
8-byte `Option<Arc<GatewayInfo>>`. Route misses keep `None`; matched requests
hold the selected immutable generation across hot reload.

Stage 2b reduced `Option<SelectedHttpBackend>` from 504 bytes to 32 bytes.
Service and AI selections now retain a validated route-generation/index handle;
Service TLS policy is captured separately, and Direct remains a boxed cold-path
compatibility variant until its synthetic backend is removed.

Stage 2c reduced the selected gRPC backend from 224 bytes to a 24-byte
generation/index handle plus captured TLS policy. The handle also carries the
exact original backend slot, removing the former duplicate-name linear lookup.

Stage 2d removed the DirectEndpoint synthetic `CompiledBackend` and the copied
`HTTPBackendRef` from request state. Direct jumps now retain only their bounded
preset and resolve the validated slot through the request-pinned route
generation. This removes the Direct-only schema clones, two Box allocations,
identity reconstruction, and empty plugin runtime allocation. AI already stores
only a generation/index handle in the base Context, so no additional AI copy
was removed. The ordinary Context remains 1,832 bytes and allocation-free.

Stage 3 moved TLS handshake facts and gRPC service/method/status/deadline facts
behind two independent lazy boxes while retaining common protocol/body gates
inline. The access-log JSON remains flat, reads never instantiate either
state, and cleartext ordinary HTTP stays allocation-free. The unused,
non-serialized `downstream_peer_port` field was removed. This reduced
`RequestInfo` from 528 to 288 bytes and `EdgionHttpContext` from 1,832 to 1,592
bytes.

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
