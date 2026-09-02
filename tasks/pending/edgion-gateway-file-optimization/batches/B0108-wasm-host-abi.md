# B0108 — Wasm HTTP Host ABI

## Scope
- Baseline: `512d485d`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/wasm/host_abi.rs`
- Baseline blob: `deb9368327e247b49a855a5f3f38b4c181bd93f4`
- Implementation allow-list: `edgion-gateway/src/plugins/http/wasm/host_abi.rs`
- Forbidden files: every file outside the owned file, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Trace the exact Proxy-Wasm host ABI surface, Wasmtime memory and trap boundaries, context/property/header/body access, local responses, callout scheduling, logging/access-log ownership, and validation parity with the runtime-independent `edgion-wasm` profile.
- Preserve panic isolation, bounded memory, canonical request-body and response-stream ownership, sensitive-data handling, observability restraint, no-tracing, and general-purpose shared mechanisms.
- Existing integration tests remain a frozen validation floor. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue.

## Handoff
- Analyst: independent read-only analysis completed at `512d485d`; the target blob remained `deb9368327e247b49a855a5f3f38b4c181bd93f4`
- Decision: change. Bound lower-trust scalar, property-path, serialized-map, header-operation, local-response-header, and shared-data decoding before large transient or retained allocations; replace quadratic case-insensitive removal-name discovery.
- Complexity: complex/plan-first. Valid guest inputs can otherwise allocate more than a gigabyte inside one non-interruptible host call while bypassing the 128 MiB guest-memory and 32 MiB retained-state bounds.
- Preserved behavior: exact host ABI/profile, lossy UTF-8 compatibility, ordered duplicate maps, property paths, ABI status classes, atomic rejection, BodyStorage/current response chunks, local-response replacement, callout/shared-data semantics, generic failure policy, no guest-byte logging, no tracing, and pooled VM lifecycle.
- Tests: deterministic inline low-limit/helper and real-host-call regressions proving early rejection and no state mutation without attack-sized allocations.
- Adjudication correction: Wasmtime transports Rust host panics across JIT frames and resumes unwinding at the typed-call boundary; existing returned-error poisoning is insufficient. This does not change the independent host-ABI allocation brief.
- Deferred: D0073 assigns redacted catch-and-poison coverage to the future `vm_manager.rs` batch. Request pseudo-header expansion remains non-actionable without an explicit compatibility requirement.
- Integration: existing frozen Wasm scenarios cover end-to-end behavior; adversarial allocation boundaries are deterministic unit concerns, so no integration issue is needed.
- Review: pending
- Verification: pending
- Commit: pending
