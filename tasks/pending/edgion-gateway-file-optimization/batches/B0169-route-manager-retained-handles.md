# B0169 — Retained Route Manager Handles

## Scope

- Candidate analysis HEAD: `bc47034c9d355a3510e5fe1d15784dd4763aed4d`; freshness-revalidated and promoted after B0168 at `656cf2c790303a0ad9bde4f3b2d664b2d615a8a6`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/grpc/routes_mgr.rs`
- Promoted target blob: `16db96bb562eb4f912a071470157d9119ed33dd4`
- Exact technical implementation allow-list:
  - `edgion-gateway/src/routes/grpc/routes_mgr.rs`
  - `edgion-gateway/src/routes/http/routes_mgr.rs`
  - `edgion-gateway/src/routes/http/tests.rs`
  - `edgion-gateway/src/routes/l4_rebuild.rs`
  - `edgion-gateway/src/routes/tcp/routes_mgr.rs`
  - `skills/04-review/routing/regression-guards/l4-rebuild-helper-clear-pass.md`
  - `tasks/todo/route-manager-retained-listener-handle-integration.md`

## Handoff

- Analysis: `b0166_final_review` read the gRPC manager and publication path, retained HTTP/L4 proxy ownership, sibling managers/helpers, tests and review precedent, then revalidated after B0168's mechanical getter removal.
- Correctness defect: full stale sweeps and HTTP/gRPC/shared-L4 orphan sweeps remove `by_listener` ownership without emptying the removed manager. Live proxy objects retain its `Arc`, so a removed listener can continue serving an old route table. Direct affected-listener removal already empties first and remains unchanged.
- Performance defect: `DomainGrpcRouteRules` stores an inner `ArcSwap<GrpcRouteRules>` that is only written during construction and is itself published through the outer manager `ArcSwap`; every request pays a redundant inner atomic load.
- Required fix: atomically apply clear-on-remove to HTTP, gRPC and the shared L4 helper, and replace only the immutable gRPC inner layer while preserving the outer publication owner.
- Plan: `b0164_final_review` proposed clearing only the exact manager Arc returned by each stale/orphan removal across HTTP, gRPC and shared L4, preserving direct affected ordering, adding deterministic injected-listener test seams, and collapsing only the immutable inner gRPC ArcSwap. Independent reviewer `b0168_final_review` approved it without Must Fix and required successful-removal counts, single PGIS snapshots under existing locks, real nonempty retained managers, and new/old instance isolation.
- Implementation: full stale and partial orphan removals now publish an empty table through the exact removed manager before dropping it; direct affected empty buckets remain clear-before-remove. HTTP/gRPC production wrappers retain their rebuild locks and one PGIS snapshot. The write-once inner gRPC route ArcSwap is now an immutable Arc while the outer per-listener ArcSwap remains the sole generation publisher.
- Tests: full routes passed 512/512. Focused gRPC manager tests passed 3/3, HTTP 31/31 and TCP 5/5; final review additionally ran TLS 4/4 and UDP 6/6. Tests seed real nonempty retained managers for full/orphan paths, prove their old Arcs empty after removal, prove recreated managers are distinct, and pin old/current gRPC generations.
- Integration gap: `tasks/todo/route-manager-retained-listener-handle-integration.md` records live cross-stream listener deletion/relist coverage; no existing integration file changed.
- Review: fresh independent final reviewer `b0169_final_review` verified all seven files, retained proxy ownership, removal/recreation concurrency, the outer publication boundary and generic L4 evidence. Verdict: LGTM with no Must Fix or Should Fix.
- Verification: `cargo fmt --all`; full routes 512/512 and focused suites above; locked workspace all-target check and Clippy; locked Gateway binary build; agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and 3,113-file frozen-path guards passed. Clippy retained only existing warnings.
- Commit: `837bb26d`; never pushed by the campaign.
