# B0102 — RequestMirror

## Scope
- Baseline: `6537bb33`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/request_mirror/{mirror_log.rs,mirror_state.rs,mod.rs,plugin.rs}`
- Revalidated blobs: `mirror_log.rs` = `614c6bd3799087df5f6a005861f5dff224aaa3b0`; `mirror_state.rs` = `ebcb4250b652f3c33bb35605bead997162502843`; `mod.rs` = `021731c4b8931601209562ca3a746fd4037f18fc`; `plugin.rs` = `22253839b30c5e342bf9cedd9cce725964b3fbd3`
- Supporting blob: `edgion-gateway/src/plugins/http/common/http_client.rs` = `0f3d4285a87715bc9afff2e5bdac6690902d80ef`
- Implementation allow-list: `edgion-gateway/src/plugins/http/request_mirror/plugin.rs`, `edgion-gateway/src/plugins/http/common/http_client.rs`, `skills/04-review/request-mirror/intentional-design/mirror-uses-separate-http-client-bypasses-lb-tls-policy.md`, `skills/01-architecture/00-common/05-unification-points.md`, and `skills/04-review/cpu-memory/regression-guards/request-mirror-reqwest-client-per-plugin-instance.md`
- Forbidden files: the other three owned files, resources, Controller, runtime/session/BodyStorage, backends and routing, other review knowledge, documentation, Cargo, every existing `edgion-tests/**` path, and protected user paths

## Analysis boundary
- Traced both route/filter construction paths, client and pool lifecycle, body snapshot/tee ownership, detached-task deadlines and cancellation, concurrency/buffering, EndpointSlice selection and address validation, egress/TLS/redirect/proxy behavior, framing and header filtering, logs/metrics/access log, tests, integration coverage, and documentation.
- Preserve round-robin selection, EndpointSlice membership and post-resolution address validation, the accepted port-443 TLS heuristic and BackendTLSPolicy boundary, sampling, semaphore, body snapshot/tee and cancellation, framing/header filtering, response dropping, canonical access log, metric labels, and main-request `GoodNext` behavior.
- Existing integration tests remain frozen.

## Handoff
- Analyst: independent read-only analysis started on `3e450400`; the intervening B0101 record commit did not change target/support blobs when promoted on `6537bb33`
- Decision: change `plugin.rs`; share one process-wide RequestMirror client through the canonical common helper instead of constructing one pool per plugin generation
- Complexity: complex/plan-first because removing the client-internal connect timeout leaves the detached writer's existing `connect_timeout + write_timeout` outer deadline as the sole timeout contract
- Plan review: approved after one Must Fix added removal of the obsolete sole-consumer pooled-client helper and its stale import/comment; no Must Fix or Should Fix remains
- Implementation: initial three-file and corrected five-file allow-lists were each honored; added the dedicated process singleton through the canonical factory, removed per-generation clients and the obsolete pooled builder, cloned the shared handle immediately before spawn, added no-network identity coverage, and corrected every directly affected knowledge record
- Implementer verification: 9 RequestMirror and 5 common HTTP-client tests, Gateway library/workspace check and Clippy, format, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff, and frozen-path guards passed
- Final-review correction: functional review passed, but canonical knowledge still referenced the deleted helper or generic client and the common module docs still described one singleton; the exact two-file knowledge expansion and stable-symbol cleanup were independently plan-reviewed and approved
- Correction implementation: exact five-file allow-list honored; module docs, canonical unification registry, F-017 case, and stable-symbol architecture references now describe the dedicated singleton while source behavior remains unchanged
- Final review: independently approved after correction with no remaining Must Fix or Should Fix; canonical client construction, singleton/thread lifetime, deadline ownership, egress/TLS/address behavior, body/task/framing/cancellation, logs/metrics, main-request behavior, and all knowledge records are consistent
- No-change candidates: `mirror_log.rs`, `mirror_state.rs`, and `mod.rs`
- Deferred: D0066 owns capacity bounds and panic prevention; D0067 owns deterministic no-network unit-test arming
- Integration: existing frozen coverage is sufficient; client identity belongs in deterministic colocated tests and no integration issue is needed
- Coverage: `plugin.rs` and the previously pending `common/http_client.rs` changed; `mirror_log.rs`, `mirror_state.rs`, and `mod.rs` examined-no-change
- Verification: 9 RequestMirror and 5 common HTTP-client tests passed in implementation and final review; format, locked workspace check/Clippy, Gateway binary build, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff, and frozen-path guards passed after the corrected review
- Commit: `10b0584d`
