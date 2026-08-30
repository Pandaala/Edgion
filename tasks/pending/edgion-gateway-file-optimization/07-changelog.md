# Gateway Optimization Changelog

This is the single concise record of actual changes across the file-by-file audit. It intentionally records outcomes and evidence without reproducing implementation detail already visible in the commits.

## Entry format

### `<batch-id>` — `<scope>`

- Outcome: optimized / reviewed-no-change / mixed
- Files reviewed: `<count and paths or cohesive directory>`
- Changes: `<concise concrete list>`
- Generality and architecture review: `<review agent verdict and resolved findings>`
- Verification: `<targeted tests and required gates>`
- Integration-test guard: `<result>`
- Commit: `<local commit hash>`

## Entries

### `UTIL-001` — `edgion-gateway/src/util/*`

- Outcome: mixed (4 optimized, 2 reviewed-no-change)
- Files reviewed: all 6 utility files
- Changes: removed the temporary PROXY v2 address-block allocation; avoided hostname-format allocation during DNS lookup while preserving raw and bracketed IPv6 literals; simplified host-port scanning; clarified canonical imports; added reverse mixed-family PP2, bracketed-literal, empty-port, and zero trusted-hop regression coverage.
- Generality and architecture review: independent review initially found bracketed IPv6 resolution regression and then bracketed IPv4 drift; both were fixed. Final verdict: approved with no remaining findings. DirectPeerIp/RemoteIp, SSRF classification, Real-IP trust, PP2 wire, and redaction contracts remain unchanged.
- Verification: utility tests 50/50; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains its pre-existing missing external Pingora-link failures and found no task-local error.
- Integration-test guard: no `edgion-tests` change
- Commit: `a909f16f`

### `CACHE-001` — LRU caches and their OIDC lookup consumer

- Outcome: mixed (3 optimized, 2 reviewed-no-change)
- Files reviewed: all 4 cache files plus `openid_connect/token_exchange.rs`
- Changes: generalized Local/Sharded LRU lookup and removal to standard borrowed keys; removed three per-request OIDC `String` allocations; added local and multi-shard `String`/`str` routing tests. TinyUFO and cache exports required no change.
- Generality and architecture review: the implementation audit initially proposed no change; independent review traced the OIDC request path and identified the allocation. Final review approved the `Borrow<Q>` contract, equality/hash-equivalent shard routing, type inference, and unchanged LRU/TinyUFO semantics with no findings.
- Verification: cache tests 54 passed/1 ignored benchmark; OIDC tests 61 passed; focused borrowed-key tests 2 passed; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains its pre-existing missing external Pingora-link failures and found no task-local error.
- Integration-test guard: no `edgion-tests` change
- Commit: `40f7eb74`

### `OBS-LOG-001` — protocol log buffers, entries, and logger construction

- Outcome: mixed (4 optimized, 3 reviewed-no-change)
- Files reviewed: all 7 files under `edgion-gateway/src/observe/logs/`
- Changes: bounded zero-length buffer entries so the position index cannot grow past the existing 128/512 limits; reserved SSL log channel capacity before JSON serialization to avoid wasted callback-path work on full/closed queues; removed the per-failure UDP status allocation by storing the existing static labels directly; isolated logger-factory tests with temporary directories; added buffer and UDP wire-contract coverage.
- Generality and architecture review: independent review found the empty-entry bound and UDP allocation after the initial implementation. Final verdict: approved with no remaining in-scope findings. Log schemas, masking, DirectPeerIp/RemoteIp meaning, `{}` serialization fallback, typed `LogType`, sender order, OnceLock behavior, and startup-only tracing remain unchanged. The reviewer routed `TlsConnMeta.sni` masking to the pending `ctx.rs` batch.
- Verification: protocol-log tests 16/16; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains its pre-existing missing external Pingora-link failures and found no task-local error.
- Integration-test guard: baseline `edgion-tests` tree unchanged
- Commit: `11d3e79c`

### `OBS-ACCESS-001` — access-log entry, projection, routing, and backfill

- Outcome: mixed (2 optimized, 3 reviewed-no-change)
- Files reviewed: all 5 files under `edgion-gateway/src/observe/access_log/`
- Changes: moved resolved AccessLogExtern values into the bounded `LogExt` store instead of copying them, while compacting oversized attacker-influenced allocations after UTF-8-safe truncation; isolated backfill tests with temporary directories; added allocation reuse and retained-capacity regression coverage.
- Generality and architecture review: independent review found that plain `String::truncate` retained oversized input capacity and fixed it. Final verdict: approved with no remaining findings. Access-log schema/order/omission/masking, diagnostic ordering and limits, AccessLogExtern overwrite-at-cap behavior, first-healthy/no-fallthrough routing, backfill at-least-once and queue-entry marking, and OnceLock initialization remain unchanged. Binding decisions against deferred serialization, raw-URI caching, transient-pressure fallback, and exactly-once backfill were preserved.
- Verification: access-log tests 44/44; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains its pre-existing missing external Pingora-link failures and found no task-local error.
- Integration-test guard: baseline `edgion-tests` tree unchanged
- Commit: `9d99d213`

### `OBS-CORE-001` — request diagnostics and integration access-log store

- Outcome: mixed (1 optimized, 2 reviewed-no-change)
- Files reviewed: `observe/access_log_store.rs`, `observe/request_diagnostics.rs`, and `observe/mod.rs`
- Changes: serialized only new-trace admission so concurrent distinct inserts cannot exceed the 10,000-entry test-store bound; kept existing-trace updates concurrent; made `total_stored` count successful writes; captured one cleanup timestamp per sweep; added concurrency, cleanup-cadence, TTL-read, and rejected-write regressions; synchronized the observability architecture note.
- Generality and architecture review: main review caught and fixed a potential self-deadlock from invoking `DashMap::retain` while holding a same-shard write guard. Independent review then approved the lock ordering and strict-capacity linearization with no findings. Request diagnostics retain append order, repeats, inline-eight allocation behavior, hard 32-entry tail-drop, and `diagnostics_full` semantics; the binding lazy-cleanup/no-background-task decision remains intact.
- Verification: access-log-store tests 12/12; request-diagnostics tests 2/2; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing missing external Pingora-link failures.
- Integration-test guard: baseline `edgion-tests` tree unchanged
- Commit: `4b1d27d5`

### `OBS-METRICS-001` — metrics registry, route handles, and lifecycle accounting

- Outcome: mixed (4 optimized, 3 reviewed-no-change)
- Files reviewed: all 5 files under `edgion-gateway/src/observe/metrics/`, plus the complete `pg_new_ctx.rs` and terminal `pg_logging.rs` lifecycle call sites
- Changes: balanced `edgion_ctx_active` through a release guard held across Pingora's exactly-once terminal logging hook; made malformed precomputed route-metric matrices fall back without panics, partial recording, or double-counting; removed test-metric type parsing allocation; added gauge-lifecycle and malformed-matrix regressions.
- Generality and architecture review: independent review rejected context fields and a context destructor because each disturbed the locked hot-context layout, then verified the zero-context-state design against the pinned Pingora `finish` → `logging` contract. Final verdict: approved with no findings. Metric names, labels, buckets, cardinality, initialization policy, access model, and test-mode gating remain unchanged. `ctx.rs` has no batch diff; its existing arm64 Rust 1.96.1 size-lock mismatch (1120 observed versus 1104 committed) is recorded for the pending context audit.
- Verification: metrics tests 29/29; terminal-logging tests 19/19; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged
- Commit: `96f8ac6a`

### `REQ-BODY-CORE-001` — request-body policy and state facade

- Outcome: mixed (3 optimized, 1 reviewed-no-change; 2 narrowly reviewed cross-cutting touchpoints remain pending for their full batches)
- Files reviewed: complete audit of `request_body/config.rs`, `mod.rs`, `state.rs`, and `state/enabled.rs`; targeted invariant review of `request_body/storage.rs` and `plugins/runtime/session_adapter.rs`
- Changes: centralized the admitted-empty SHA-256 value without per-request hashing; returned each mutation publication's own committed generation while still under its write lock; atomically retained modified framing length before releasing plugin-readable body access; classified missing mutation storage as `NotBuffered`; corrected facade documentation; added state, snapshot, generation, framing, and digest regressions.
- Generality and architecture review: independent review found the publication race, brittle release ordering, and false incomplete-stream classification, then approved the final diff with no remaining findings. Complete-body capture stays lazy and bounded; snapshots remain generation-stable; mutation failures publish nothing; physical caps and storage deadlines remain canonical; no response-body storage, eager spill materialization, context field, hot-path tracing, or new allocation was added. A nonblocking 32-bit `ai_guard` cap cast is routed to that plugin's pending full audit; the larger Backend/facade flattening was deliberately rejected to avoid an unproven context-layout refactor.
- Verification: request-body tests 91/91; focused bodyless digest, request/retry framing, and response release tests; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged
- Commit: `a408d7fd`

### `REQ-BODY-OBSERVER-001` — bounded synchronous request-body observer seam

- Outcome: optimized (1 file)
- Files reviewed: complete audit of `request_body/observer.rs`; targeted lifecycle review of its context slot, runtime installation, post-mutation dispatch, mirror ordering, and terminal logging consumers
- Changes: made observer budget exhaustion monotonic by atomically latching and zeroing every request-wide counter after an oversized or post-exhaustion semantic/callback charge; added oversized-to-smaller/zero re-entry and active-registry destructor-panic regressions.
- Generality and architecture review: independent review found the non-monotonic semantic-work budget defect and approved the frozen one-file fix with no remaining findings. Slot, byte, event, work, and scratch caps remain hard; retry resets semantic state without replenishing request-wide work; observation stays synchronous, borrowed, post-mutation, pre-mirror, failure-open, and allocation-free in the host chunk path; factory/callback/query/reset/finalize/drop panics remain redacted and isolated; one static payload-free outcome is drained idempotently during terminal logging.
- Verification: observer tests 19/19; all request-body tests 93/93; independent broader observer/install/context tests 11/11; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `be461374`

### `REQ-BODY-STORAGE-001` — bounded request-body storage, replay, and mutation

- Outcome: optimized (2 files)
- Files reviewed: complete audit of `request_body/storage.rs` and `request_body/storage_io_tests.rs`; targeted review of storage error consumers, replay/framing ownership, and the canonical storage-I/O deadline contract
- Changes: made replay and batched mutation splicing linear in segment count; bounded blocking scans with the canonical scan semaphore; reused one disk-scan scratch buffer and materialized disk-backed mutations directly into one final allocation; preflighted scan and temporary segment bounds; classified finalized spill failures as `StorageFailure`; made healthy empty writes and byte-identical/no-op mutations avoid I/O, generation publication, and reframing; added snapshot deadline/cursor, maximal-segment retry, many-edit, multi-chunk truncation/materialization, poisoned-state, and no-op regressions.
- Generality and architecture review: independent correctness review first found the spill-error misclassification, missing snapshot deadline coverage, and unbounded memory-scanner submission; a focused performance review then found quadratic replay/splicing, repeated disk-scan allocations, materialization double-copying, transient over-cap vectors, and false identity publication. Final review approved the two-file diff with no remaining findings. Physical capture/mutation caps, generation CAS and rollback, anonymous lazy spill ownership, shared storage-I/O admission/execution deadlines, late-task permit ownership, cancellation-safe peek/consume, retry snapshot stability, digest generation isolation, and request-only storage remain intact.
- Verification: storage tests 51/51; all request-body tests 104/104; independent request-body-filtered tests 161/161 and body-framing test 1/1; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `ae283e46`

### `REQ-BODY-OPS-001` — streaming request-body text and JSON scanners

- Outcome: optimized (2 files)
- Files reviewed: complete audit of `request_body/operation.rs` and `request_body/body_scanners.rs`
- Changes: replaced lossy-text full-chunk copying with a fixed four-byte UTF-8 carry; represented text predicates as bytes and returned known-empty constants directly; borrowed JSON object keys; bounded JSON serialization while it is produced instead of materializing an unbounded intermediate; transferred capture values without cloning; continued draining after an over-limit capture; corrected the maximum-index boundary; and added malformed/split UTF-8, predicate, and JSON regressions.
- Generality and architecture review: independent review approved the final scanner semantics and bounds with no blocking findings. Invalid JSON remains `Missing`, duplicate selected keys remain last-wins, body scan limits remain exact, and no response aggregation or new hot-path tracing was added. A nonblocking architectural follow-up remains: selected JSON objects use a `BTreeMap`, whose per-entry metadata can amplify resident memory for many tiny fields even though retained payload and string length are bounded; changing that representation requires a separately reviewed semantic-preserving design.
- Verification: request-body tests 113/113; independent broader request-body tests 170/170; JSON-select tests 10/10; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `f36d7576`

### `REQ-BODY-BLOCKING-001` — canonical blocking-scan admission and owned JSON input

- Outcome: optimized follow-up (4 files; one DSL file remains pending for its full batch)
- Files reviewed: focused cross-file review of `request_body/storage.rs`, `request_body/state/enabled.rs`, `request_body/body_scanners.rs`, and the DSL `json_get` string-input call site in `plugins/http/dsl/lang/vm.rs`
- Changes: centralized blocking scanner execution behind the existing 32-lane scan semaphore; kept the permit for the full blocking closure; moved known-empty blocking `finish` work off the async runtime; parsed owned DSL string input directly on the shared blocking pool without a channel, dedicated OS thread, or body copy; and removed the unused alternate selected-duplicate rejection machinery while retaining the production last-wins contract.
- Generality and architecture review: independent review approved the final diff with no findings. Admission occurs before blocking work starts, join failures remain the existing generic incomplete-body error, JSON admission remains independently bounded, exact invalid/limit/last-wins semantics are preserved, and the DSL builtin's established error shape is unchanged.
- Verification: canonical-admission, known-empty, owned-input, and DSL regressions passed; body-scanner tests 29/29; request-body tests 116/116; JSON-get tests 6/6; independent broader request-body tests 173/173; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `1eb48138`

### `SSE-CHAIN-001` — canonical SSE framing, retained slices, and semantic window audit

- Outcome: mixed (1 optimized, 2 reviewed-no-change with one separately tracked complex follow-up)
- Files reviewed: complete audit of `sse/chain.rs`, `sse/mod.rs`, and `sse/window.rs`; targeted guardrail semantic-window lifecycle review
- Changes: marked normalized retained-slice prefixes so later decoder callbacks normalize only newly retained handles instead of cloning and recopying every prior `Bytes`; preserved pointer identity and atomic failure; added near-handle-cap, failed-suffix, no-op, coalesce, and reset regressions. `sse/mod.rs` framing, malformed-input degradation, borrowed event view, raw identity, split handling, and EOS behavior required no change.
- Generality and architecture review: independent review approved the chain fix and confirmed it removes quadratic prefix copying without changing the canonical wire or bound contracts. The same audit found six coupled `RawWindow` continuity/residency/planning defects. Two attempted candidates were rejected or stopped because strict pre-allocation accounting remained incomplete and the change became disproportionately complex. Per the complexity policy, all window/guardrail edits were reverted and the complete design/test contract was recorded in `tasks/todo/sse-window-strict-residency-and-continuity.md` instead of landing a partial fix.
- Verification: SSE tests 62/62; gateway all-target check; `cargo fmt --all -- --check`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `4327da2b`

### `CONFIG-001` — GatewayClass and EdgionGatewayConfig snapshot handlers

- Outcome: mixed (1 optimized, 4 reviewed-no-change)
- Files reviewed: all 5 files under `config/`, covering the root exports, GatewayClass store/handler, and EdgionGatewayConfig immutable snapshot/handler
- Changes: precomputed the deterministic lowest-store-key EGC DNS resolver winner when publishing the immutable snapshot, replacing a full config-map scan on every ExternalName or routed-external upstream selection; added winner, partial-update replacement, loaded-value independence, and empty full-set reset coverage.
- Generality and architecture review: independent review approved the one-file change with no findings. Request-body sizes/timeouts and RemoteIp headers remain resolved and atomically published off the request hot path; full and partial writes remain serialized; empty full sets clear the snapshot and restore process-global defaults; GatewayClass authoritative reads remain in `ConfigClient`; logs expose counts/names and validation categories only, never specs or resolved certificate material. Returning an owned DNS config still performs one bounded winner clone; changing consumers to an Arc API was rejected as unnecessary cross-batch churn.
- Verification: config tests 91/91; focused DNS snapshot test 1/1; `cargo fmt --all -- --check`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `4621d075`

### `CONF-SYNC-CACHE-001` — Gateway conf-sync typed cache and watch dispatch

- Outcome: mixed (1 optimized, 3 reviewed-no-change; one complex recovery defect separately tracked)
- Files reviewed: all 4 files under `conf_sync/cache_client/`, covering typed cache access, cache state and compressed updates, list/watch dispatch, and module exports
- Changes: consumed each already-owned watch envelope and moved its JSON `data` subtree into typed deserialization instead of deep-cloning a potentially message-sized resource; applied the same ownership path to generic resources and `EdgionBackend`; added a pointer-identity regression that prevents reintroducing the deep copy.
- Generality and architecture review: independent review approved the ownership change with no findings. Event-kind mapping, event-level sync-version with response fallback, resource preparse/key derivation, backend raw-key validation, malformed exact-key withdrawal, and unsafe-batch fail-close behavior remain unchanged; no resource or secret payload is newly logged. The audit confirmed that malformed generic watch events can be skipped after a valid prefix is published while the stream advances. Correct recovery requires an atomic batch/failure/readiness/relist design, so no partial state-machine patch was made; the complete requirements and acceptance tests are recorded in `tasks/todo/conf-sync-generic-watch-malformed-event-recovery.md`.
- Verification: cache-client tests 40/40; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `21b01524`

### `CONF-SYNC-CLIENT-001` — Gateway conf-sync connection, rotation, and certificate watch

- Outcome: mixed (1 optimized, 4 reviewed-no-change; two complex correctness defects separately tracked)
- Files reviewed: all 5 files under `conf_sync/conf_client/`, covering certificate-file watching, typed cache aggregation/readiness, gRPC channel lifecycle, global reload generation, and endpoint rotation
- Changes: replaced the complete Controller address list in the startup INFO event with a bounded address count while retaining the selected `initial_addr` and TLS state needed for operations. No connection, retry, readiness, or security behavior changed.
- Generality and architecture review: independent review approved the log minimization and all per-file dispositions. Missing/invalid mTLS remains fail-close except for explicit `skip_tls`; certificate reload still validates before publication; cache clients and the meta handle are installed before generation cancellation. The audit confirmed two coupled defects that are too stateful for local patches: `(mtime, length)` certificate signatures can miss same-metadata content replacement in both Controller and Gateway watchers, and endpoint rotation can advance cursor/lap state before channel installation while a concurrent TLS reload can reinstall a stale pre-lock address. Their shared designs and acceptance tests are recorded in `tasks/todo/conf-sync-cert-watch-content-signature.md` and `tasks/todo/conf-sync-rotation-transactional-address-selection.md`; runtime code was intentionally left unchanged.
- Verification: conf-sync conf-client tests 21/21; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `9ca73353`

### `CONF-SYNC-ROOT-001` — conf-sync module exports and global client publication

- Outcome: reviewed-no-change (1 file)
- Files reviewed: `edgion-gateway/src/conf_sync/mod.rs`
- Changes: none. The process-global `OnceLock<Arc<ConfigClient>>` already provides atomic one-time publication, rejects duplicate initialization without replacing the live client, and returns shared ownership without exposing mutable global state.
- Generality and architecture review: implementation and independent reviews approved the file with no findings. Module exports remain narrow, and the file has no logging, secret-bearing data, schema, serialization, task, or request-hot-path behavior. Supporting replacement would require a separately justified global lifecycle/task-ownership design; no current defect requires one.
- Verification: static call-site review; adjacent conf-sync cache-client tests 40/40 and conf-client tests 21/21; current workspace check, workspace clippy, SSA, metrics inventory, Pingora source, and gateway no-tracing guards passed. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: no code commit; tracker-only closure
