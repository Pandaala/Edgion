# B0148 — Local Rate Gate

## Scope

- Baseline and promotion HEAD: `1db7a23e4845d9d3dbc23f004b3c55aa2290422b`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs`
- Necessary support file: `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs`
- Baseline blobs: owned `01194591e00023fa52aa120c2e7b24f5756ada91`; support `b4d33164`
- Exact implementation allow-list: owned and support files only

## Handoff

- Analysis: `b0142_test_helpers_analysis` traced every gate caller, registry and hot-reload behavior, refill arithmetic, Stage 1 paths, Redis time ownership, metrics, failure policy and tests. Decision: change; moderate monotonic-clock correction without plan-first complexity.
- Value: prevent a backward wall-clock adjustment from freezing process-local connection-token refills. The local gate now owns a process-lifetime monotonic origin, while the shared Redis limiter retains absolute Unix epoch milliseconds for server-side windows.
- Preserved contracts: rate/capacity and interval arithmetic, zero timestamp sentinel, registry identity and no-eviction policy, runtime-generation updates, Stage ordering, Redis behavior, metrics, denial codes, configured failure policy and `DirectPeerIp` source keys.
- Deferred: D0100 owns the high-severity refill/consume/update linearizability and per-connection global registry-mutex design. D0028 retains numeric-domain bounds and [D0099](../../../../skills/04-review/listeners/intentional-design/bounded-serial-connection-admission.md) retains listener accept-loop scheduling; none is bundled here. Existing integration tests remain frozen.
- Implementation: added an `Instant` origin to each `LocalRateGate`; the production acquisition method samples once with `saturating_duration_since`, clamps elapsed milliseconds to `u64`, preserves the nonzero sentinel and delegates to the existing arithmetic seam. The plugin caller no longer samples `SystemTime`; the deterministic timestamp seam is test-only. A production-entry capacity test was added without sleeps.
- Review: `b0135_final_review` independently approved the production change and Redis epoch separation. It requested changing the new test interval from 60 seconds to `u64::MAX` so an extreme scheduler pause cannot refill it; the corrected test received LGTM with no remaining Must Fix or Should Fix. The reviewer independently confirmed D0100 as non-duplicate, high severity and plan-first.
- Verification: all 6 local-gate and 3 connection-rate-limit plugin tests passed serially. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and 3,093-file frozen integration guards passed; Clippy retained only existing warnings.
- Commit: `07c72099`; local only, never pushed by the campaign.
