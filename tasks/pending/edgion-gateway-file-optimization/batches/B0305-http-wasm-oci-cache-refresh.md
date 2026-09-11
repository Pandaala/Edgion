# B0305 — HTTP Wasm OCI Cache Refresh

## Scope

- Baseline: `26e7ec7f`.
- Pipeline state: terminal candidate; changed and independently approved, with commit-gate verification active.
- Active anchor crate: `edgion-gateway`.
- Cohesive targets: `plugins/http/wasm/oci/cache.rs`, `cache_tests.rs`, `memory_pipeline_tests.rs`, `mod.rs`, and `plugins/http/wasm/vm_manager.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- External `7534a9bf` rewrote the OCI cache, added separate cache/memory-pipeline tests, changed OCI orchestration and adjusted VM-manager ownership. The five files form one artifact-fetch, validation, caching and compilation boundary.
- The analyst must inspect OCI reference/auth/egress/TLS resolution, response/archive bounds, digest and manifest verification, cache identity/eviction/singleflight, filesystem and memory ownership, Wasmtime compilation/generation lifecycle, redaction/logging/metrics and deterministic test quality before deciding change, examined-no-change or defer.
- Revalidate the prior B0110/B0111/B0115 evidence and current Wasm/OCI review knowledge against the external rewrite. Existing integration tests remain frozen.

## Analysis result

- Analyst `b0305_wasm_oci_analysis` read all five targets and traced registry auth/redirect/egress, capped fetch and archive validation, cache ownership/singleflight, compilation permits, VM generation publication and deterministic tests.
- Decision: `oci/mod.rs` has one simple security fix. Allowed blob redirects and Bearer realms can place a remote-controlled query-bearing final URL in `reqwest::Error`; three error mappings format that URL into a load error later emitted by VM-manager WARN logs.
- Exact technical allow-list: `oci/mod.rs`. Strip stored URLs before formatting transport/body errors and add deterministic redirect/realm regressions without changing classification, registry context or retry semantics.
- `cache.rs`, `cache_tests.rs`, `memory_pipeline_tests.rs` and `vm_manager.rs` are examined-no-change candidates. Current memory accounting, owner generations, claim cancellation, bounded permits, analysis/compilation and atomic publication remain coherent.
- Three support docs still describe a disk cache and may be corrected to the bounded process-memory cache: `skills/01-architecture/02-gateway/01-startup-shutdown.md`, `skills/02-features/03-resources/13-edgion-plugins.md`, and `docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md`.
- Fresh analysis verification passed OCI 61/61 and VM-manager 60/60. Existing integrations remain frozen and sufficient.

## Implementation

- Implementer `b0305_wasm_oci_implement` removes stored request URLs before formatting all four OCI reqwest send/body errors. Safe initial registry context and all classification, status, retry, redirect, egress, cap, cache and VM behavior are unchanged.
- Two bounded local raw-HTTP regressions cover a query-bearing allowed blob redirect and Bearer realm through deliberately truncated response bodies. Calls and fixture joins have two-second bounds; static failure messages cannot echo the tested secret.
- Three support docs now describe the bounded content-addressed process-memory cache instead of an obsolete disk cache.

## Independent review

- Reviewer `b0305_wasm_oci_review`: final LGTM with no Must Fix, Should Fix or deferred finding after the implementer replaced a released-port fixture, removed secret-bearing assertion formatting and directly covered both body-stream mappings.
- The reviewer independently confirmed all four production mappings, preserved classifications/context and the four examined-no-change target dispositions.

## Verification

- Focused redaction tests passed 2/2, all OCI tests passed 63/63 and VM-manager tests passed 60/60.
- Gateway locked all-target check, formatting, Gateway no-tracing, English-only and diff checks passed during implementation/review.
- Applied format plus locked workspace all-target check and Clippy passed. Agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards all passed; Clippy retained only existing warnings.
- Existing integration files were not modified or run.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic local commit.
