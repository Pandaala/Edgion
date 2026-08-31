# Gateway Optimization Changelog

This is the single concise record of actual changes across the file-by-file audit. It intentionally records outcomes and evidence without reproducing implementation detail already visible in the commits. Historical entries retain the original terms `optimized` and `reviewed-no-change`; they map mechanically to the current ledger states `changed` and `examined-no-change`.

## Entry format

### `<batch-id>` — `<scope>`

- Outcome: changed / examined-no-change / defer
- Files reviewed: `<count and paths or cohesive directory>`
- Changes: `<concise concrete list>`
- Generality and architecture review: `<review agent verdict and resolved findings>`
- Verification: `<targeted tests and required gates>`
- Integration-test guard: `<result>`
- Commit: `<local commit hash>`

## Entries

| Batch | Date | Files | Outcome | Actual improvement | Verification | Commit | Follow-ups |
|-------|------|-------|---------|--------------------|--------------|--------|------------|
| [`B0033`](batches/B0033-backends-circuit-breaker.md) | 2026-08-31 | 2 | 1 changed, 1 examined-no-change | Removed the contention test's intentional permit leak and proved concurrent RAII release to zero; normalized the resumed campaign artifacts and froze existing integration-test contents | 4 focused tests; format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `770be849` | Existing [atomic Service snapshot task](../../todo/backend-traffic-policy-atomic-service-snapshot.md) retained |
| [`B0034`](batches/B0034-backend-discovery-shared.md) | 2026-08-31 | 1 | changed | Replaced repeated removed-address snapshot scans with a first-occurrence index, reducing cleanup from O(E×R) to O(E+R) while preserving duplicate and missing-entry semantics | 2 focused tests; format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `6b3b679b` | none |
| [`B0035`](batches/B0035-backend-dns-cache.md) | 2026-08-31 | 1 | defer | Left runtime code unchanged because complete ExternalName DNS cache identity, Service update/delete invalidation, and bounded retention require one wider design | Format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `16d39e25` | [`D0001`](deferred/D0001-external-name-dns-cache-identity-lifecycle.md) |
| [`B0036`](batches/B0036-backend-discovery-module.md) | 2026-08-31 | 1 | examined-no-change | Retained the minimal discovery façade and its public re-exports; pruning or import normalization offered only API risk and style churn | Format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `1b0e8c11` | none |
| [`B0037`](batches/B0037-backend-endpoint-slice.md) | 2026-08-31 | 4 | 1 changed, 1 examined-no-change, 2 deferred | Removed request-path EndpointSlice backend-construction tracing; retained lifecycle behavior and recorded the same-key ownership/reverse-index/cleanup redesign separately | 15 focused tests; format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `353268ec` | [`D0002`](deferred/D0002-endpoint-slice-ownership-transitions.md) |
| [`B0038`](batches/B0038-backend-service-discovery.md) | 2026-08-31 | 3 | examined-no-change | Retained atomic Service snapshots, real incremental updates, lock-free request reads, explicit-delete cleanup, policy retention, and accepted cross-kind convergence | Format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `62398c46` | D0001 and D0002 retained |
| [`B0039`](batches/B0039-backend-tls-store.md) | 2026-08-31 | 1 | defer | Left runtime code unchanged because authoritative removal of omitted Gateway client identities requires a coordinated store and full-set lifecycle change | Format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `abca60f3` | [`D0003`](deferred/D0003-gateway-backend-tls-authoritative-replacement.md) |
| [`B0040`](batches/B0040-backend-health-check.md) | 2026-08-31 | 6 | 2 changed, 3 examined-no-change, 1 deferred | Denied HTTP health-probe redirects and replaced dependency/server error formatting with fixed failure classes; retained scheduling/status/TLS architecture and recorded the wider designs separately | 32 focused tests; format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `31d8bcdd` | [`D0004`](deferred/D0004-health-task-status-generation-ownership.md), [`D0005`](deferred/D0005-active-health-check-tls-contract.md) |
| [`B0041`](batches/B0041-backends-module.md) | 2026-08-31 | 1 | changed | Borrowed ExternalName Service annotations instead of cloning the complete map on every request while preserving DNS mode and resolver precedence | 14 focused tests; format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `c3ff57b5` | D0001 retained |
| [`B0042`](batches/B0042-backend-outlier-detection.md) | 2026-08-31 | 4 | 2 changed, 2 examined-no-change | Replaced roughly 6.4 seconds of deadline sleeps with deterministic test-only expiry while strengthening recovery, half-open, ejection-count, and backoff assertions | 34 focused tests in 0.03s after compilation; format, check, Clippy, SSA, metrics, Pingora source, no-tracing passed; 13 known external doc links remain missing | `803d9342` | [`D0006`](deferred/D0006-outlier-generation-gauge-lifecycle.md) |
| [`B0043`](batches/B0043-backend-peer-builder.md) | 2026-08-31 | 1 | changed | Preserved the complete PEM-ordered upstream mTLS client certificate chain and removed avoidable SNI and `appProtocol` peer-construction allocations | 9 focused tests; format, check, Clippy, SSA, metrics, Pingora source, no-tracing, and frozen paths passed; 13 known external doc links remain missing | pending automatic commit | Existing BackendTLSPolicy publication and backend-port tasks retained; no duplicate finding |

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

### `TLS-CORE-001` — host FIPS detection and downstream approved profile

- Outcome: mixed (1 optimized, 2 reviewed-no-change)
- Files reviewed: `tls/fips_host.rs`, `tls/fips_profile.rs`, and `tls/mod.rs`
- Changes: replaced the two runtime `CString` constructions used for the approved cipher and curve lists with compile-time NUL-checked static C strings, removing two heap allocations from every downstream TLS handshake when the FIPS policy is active; retained the public profile strings and added an exact parity regression so the Rust and FFI representations cannot drift.
- Generality and architecture review: independent review approved the one-file change with no findings. FFI pointers have static lifetime, non-success returns remain strict fail-close, and only impossible interior-NUL error branches were removed. Genuine BoringSSL FIPS state, host-kernel detection, tri-state policy precedence, explicit operator cipher override, approved TLS 1.2 values, curve enforcement, metrics/logging, TLS 1.3 limitations, upstream limitations, and compliance wording remain unchanged. Unsupported Gateway rustls parity was deliberately excluded.
- Verification: TLS/FIPS tests 20/20, including live FFI profile application and TLS 1.2 approved/ChaCha20 negotiation coverage; `cargo fmt --all`; workspace check; workspace clippy (exit 0, pre-existing warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `9ab98fd0`

### `TLS-BORINGSSL-001` — downstream mTLS verification callback safety

- Outcome: mixed (1 optimized, 1 reviewed-no-change)
- Files reviewed: `tls/boringssl/mod.rs` and `tls/boringssl/mtls_verify_callback.rs`
- Changes: moved both BoringSSL callback boundaries to the shared redacted panic catcher; made missing callback index/context fail closed; and made repeated ex-data replacement transactional so a failed `SSL_set_ex_data` preserves the live old allocation instead of leaving a dangling slot.
- Generality and architecture review: independent review approved the final diff. Panic hooks and nested panic-payload destructors cannot expose dynamic certificate/dependency detail or unwind across C; callback panics and impossible missing-state cases reject the handshake; successful and failed ex-data replacement have single, explicit ownership. SAN/CN values remain comparison inputs with the existing sensitive-mode rendering, normal certificate-chain and allowlist semantics are unchanged, and the module export remains narrow. The analogous `SslCtx` lifecycle is intentionally left for the immediately following `tls-runtime` batch so its complete state machine can be audited together.
- Verification: mTLS validation tests 6/6; BoringSSL module compiled through the gateway library test target; `cargo fmt --all`; workspace check; workspace clippy (exit 0, 929 pre-existing gateway warnings); SSA, metrics inventory, and Pingora source guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `f33146ef`

### `TLS-RUNTIME-001` — TLS callback context ownership and certificate extraction

- Outcome: mixed (2 optimized, 4 reviewed-no-change; one cross-callback refactor separately tracked)
- Files reviewed: all 6 files under `tls/runtime/`, covering backend callback wiring and certificate extraction, Gateway callback state, and module exports
- Changes: protected the `SslCtx` BoringSSL destructor with the shared redacted panic boundary; made ex-data replacement release the displaced `Box` only after a successful store; made take clear the slot before reconstructing ownership; encoded SHA-256 certificate fingerprints directly into one preallocated string instead of allocating one string per byte plus an intermediate vector; and replaced an empty placeholder test with real DNS/IP SAN extraction coverage plus exact fingerprint-format coverage. TLS architecture docs now name the canonical panic helper.
- Generality and architecture review: independent review approved the final diff. Store failure preserves the old live slot, take failure leaves ownership with BoringSSL, panic hooks and payload destruction stay inside the FFI boundary, certificate and SAN data are not newly logged, and fingerprint format/fallback semantics are unchanged. The audit also confirmed that an owned `SslCtx` handoff could remove full resolved-Secret clones and that injected finalize-store failure lacks an exactly-once log owner. Because that change crosses select, certificate, completion, and destructor callbacks, it was not implemented; the required state matrix and fault-injection proof are recorded in `tasks/todo/tls-ssl-ctx-owned-handoff-and-exactly-once-audit.md`. Unsupported Gateway rustls parity remained excluded.
- Verification: certificate extraction tests 2/2 and TLS callback helper tests 9/9; `cargo fmt --all`; workspace check; workspace clippy (exit 0, 929 pre-existing gateway warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `0090e5bf`

### `TLS-STORE-001` — TLS store sharing, deterministic matching, and generation safety

- Outcome: mixed (2 optimized, 2 reviewed-no-change; two cross-generation designs separately tracked)
- Files reviewed: all 4 files under `tls/store/`, covering the lock-free SNI matcher, conf-sync adapter, module exports, validation/LKG storage, and matcher rebuild
- Changes: retained conf-sync `Arc<EdgionTls>` allocations instead of deep-cloning resources and inline certificate/private-key buffers; replaced certificate validation detail and unbounded host/parent/collision lists in config logs with stable reason/count fields; canonicalized hostname case before collision grouping and collapsed same-resource case duplicates; and made compressed `add` replace an older key generation even when the new certificate is invalid, while ordinary invalid `update` continues to retain last-known-good. Added pointer-identity, case-order independence, duplicate-host, and delete-to-create regressions.
- Generality and architecture review: independent review approved the final diff. Handshake lookup remains lock-free, exact and RFC 6125 wildcard semantics are unchanged, Arc lifetimes remain valid after snapshot guards drop, invalid recreated resources no longer retain old host/listener/certificate side effects on the partial path, and no certificate identity or parser detail is newly logged. The audit confirmed two changes that require broader designs: full snapshots need a Kubernetes/FileSystem/etcd resource-identity contract before LKG can distinguish update from recreate, and the delta-shaped store API still clones/rebuilds globally while data and matcher publish through separate generations. These are recorded in `tasks/todo/tls-last-known-good-resource-identity.md` and `tasks/todo/tls-store-true-incremental-atomic-publication.md`; no partial cross-state-machine fix was made. Unsupported Gateway rustls parity remained excluded.
- Verification: TLS store tests 11/11; `cargo fmt --all`; workspace check; workspace clippy (exit 0, 929 pre-existing gateway warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `19d6e382`

### `TLS-VALIDATION-001` — bounded and redacted TLS identity validation

- Outcome: mixed (3 optimized, 2 reviewed-no-change; two complex mTLS follow-ups separately tracked)
- Files reviewed: all 5 files under `tls/validation/`, covering the shared certificate-validation shim, module exports, downstream client SAN/CN allowlists, backend SAN matching, and outbound rustls SAN-pinning error recovery
- Changes: replaced backend SAN result strings containing certificate and policy identities with fixed static outcomes, removing success-path formatting and keeping mismatch diagnostics generic; corrected `allowedCns` to the documented case-sensitive exact-match contract instead of applying DNS wildcard/case-folding semantics; and made typed outbound error-chain traversal carry one global 16-node budget through `io::Error::get_ref()` hops, suppressing the recursive-display fallback once that budget is exhausted. Added exact CN, wildcard rejection, case mismatch, fixed-reason, and nested-`io::Error` depth regressions.
- Generality and architecture review: independent review approved the final diff and both no-change dispositions. The common certificate validator remains the Controller/Gateway canonical implementation, module exports remain narrow, backend hostname and URI matching behavior is unchanged, the base outbound certificate verifier still runs before additional SAN pinning, and no certificate identity is newly exposed. Type-aware downstream mTLS SAN matching needs an operator-facing syntax and shared validation contract, while leaf-only BoringSSL allowlist evaluation crosses per-depth callback and exactly-once audit behavior; both were intentionally left unchanged and recorded in `tasks/todo/mtls-typed-allowed-san-semantics.md` and `tasks/todo/mtls-leaf-only-whitelist-verification.md`.
- Verification: TLS validation tests 26/26; `cargo fmt --all`; workspace check; workspace clippy (exit 0, 929 pre-existing gateway warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `f65f66bc`

### `BACKENDS-POLICY-TLS-001` — allocation-free backend TLS policy lookup

- Outcome: mixed (1 optimized, 2 reviewed-no-change; three cross-module follow-ups separately tracked)
- Files reviewed: all 3 files under `backends/policy/backend_tls/`, covering the immutable policy/reverse-index snapshot, conf-sync full/partial adapter, module exports, typed target arbitration, and request-path lookup
- Changes: reduced the exactly-one-target eligibility path from a temporary `Vec` to `Option`, and made request-path exact-section/whole-target lookup consume the already-canonical reverse-index proof instead of rebuilding owned `PolicyTargetKey`s for every candidate. Added multiple-targetRef and zero-allocation lookup regressions. A publication-only identity-validation filter was evaluated and then removed because the current `Option<Arc<BackendTLSPolicy>>` lookup API would collapse invalid and absent states, allowing callers to interpret an invalid policy as permission to construct a plaintext peer.
- Generality and architecture review: independent review approved the final diff, the corrective removal, and both no-change dispositions. Policies and reverse-index buckets still publish atomically in one immutable generation; equivalent policies retain `Arc` identity; Service and EdgionBackend targets remain typed and isolated; exact section still wins before whole-Service fallback; candidate order remains oldest `creationTimestamp` then name; count-only config logs expose no hostname, CA, client identity, Secret, or spec. A complete absent/effective/invalid lookup contract with HTTP, gRPC, and AI fail-close propagation is recorded in `tasks/todo/backend-tls-policy-defensive-validation-fail-closed.md`; the unsafe partial filter was not retained. The delta handler still clones the full policy map and rebuilds the full reverse index, so the required target-incremental immutable publication design is recorded in `tasks/todo/backend-tls-policy-true-incremental-publication.md`. While running the canonical resource tests, the audit also confirmed that the repository still carries a forbidden BackendTLSPolicy v1alpha3 fallback/conversion path; its atomic Controller/resources/docs removal is recorded in `tasks/todo/backend-tls-policy-v1-only-atomic-promotion.md` and was not partially changed here.
- Verification: backend TLS store/handler tests 12/12 after the corrective removal; adjacent backend policy tests 4/4; shared BackendTLSPolicy resource tests 13/13; `cargo fmt --all`; workspace check; workspace clippy (exit 0, 929 pre-existing gateway warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commits: `f4acade9`, `c34041f3`

### `BACKENDS-POLICY-ROOT-001` — typed backend TLS policy lookup boundary

- Outcome: reviewed-no-change (1 file)
- Files reviewed: `backends/policy/mod.rs`, including its HTTP and gRPC callers and the shared Service port-name resolver
- Review: retained the typed `LogicalBackendHandle` proof boundary, denied-reference rejection, and exact-section-before-whole-target lookup. Independent review found that a requested numeric Service port whose name cannot be resolved is currently indistinguishable from an omitted port: the wrapper falls through to whole-Service lookup, and a missing whole-target policy may become a plaintext peer. Correcting this requires an explicit absent/effective/invalid-or-unresolved result propagated through HTTP, gRPC, and AI peer construction, so no partial code change was made; the work is included in `tasks/todo/backend-tls-policy-defensive-validation-fail-closed.md`.
- Verification: existing adjacent backend policy tests 4/4; static caller review confirmed the current `None` plaintext interpretation and the test gap around real Service snapshot skew. No code changed in this batch.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`

### `BACKENDS-POLICY-TRAFFIC-001` — backend traffic policy publication and retry accounting

- Outcome: reviewed-no-change (5 files; three complex follow-ups separately tracked)
- Files reviewed: all 5 files under `backends/policy/backend_traffic/`, covering bounded authority composition, conf-sync ownership/logging, exports, retry-budget window accounting, typed winner arbitration, request-facing compatibility maps, immutable AI capture, and full/partial publication
- Review: retained the authority path's fixed suffix, DNS-label validation, IP-literal rejection, and static failure labels; the conf handler's borrowed-to-owned full-set clone and metadata-only logs; typed Service/EdgionBackend isolation; oldest-then-name arbitration; old/new target clearing; and equivalent-config retention of live retry/circuit/outlier handles. No isolated refactor could preserve all current semantics. Three independently confirmed findings were therefore documented instead of partially changed: `tasks/todo/retry-budget-generation-and-window-coherence.md` covers partially visible counter rotation, multiple clock samples, stale history after multi-window idle, and Service generation drift; `tasks/todo/backend-traffic-policy-atomic-service-snapshot.md` covers mixed-generation Service maps, omitted Service snapshot fields, and indexed publication; `tasks/todo/upstream-authority-bounded-template-rendering.md` covers enforcing the 63-byte label bound before allocation while preserving missing-variable semantics.
- Verification: backend traffic policy tests 24/24. Static call-site review covered HTTP connect/status/timeout retries, AI dispatch capture, Service authority/connect/circuit lookups, health-check authority capture, and the shared template evaluator. No runtime code changed in this batch.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`

### `BACKENDS-PORT-RESOLVER-001` — selected Service endpoint port resolution

- Outcome: reviewed-no-change (1 file; complex cross-consumer fix separately tracked)
- Files reviewed: `backends/port_resolver.rs`, its Service/EndpointSlice stores, BackendTLSPolicy wrapper, and HTTP, TCP, TLS, UDP, and request-mirror callers
- Review: preserved the closed heterogeneous-rollout behavior that resolves a named port against the selected endpoint's originating slice and compares parsed IPv4/IPv6 addresses numerically. Static review found that the request path otherwise deep-clones the Service, takes two blocking EndpointSlice read locks, deep-clones all slices, and scans/parses all endpoint addresses. Missing or ambiguous Service/port/slice/origin data silently retains the load balancer's first port, the unknown-origin fallback can select another slice's minimum port, and unchecked `i32 as u16` conversions accept invalid transport values. `resolve_port_name` also collapses an omitted backend port and failed Service lookup into the same `None`, which can erase an exact BackendTLSPolicy attachment. A safe fix requires immutable selector-carried port identity plus typed failures propagated through every protocol, so no partial cast or fallback patch was made; the complete work is recorded in `tasks/todo/backend-port-resolution-precompiled-fail-closed.md` and coordinated with the BackendTLSPolicy fail-close task.
- Verification: existing port-resolver test 1/1; static caller and store review. No runtime code changed in this batch.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`

### `BACKENDS-PANIC-THRESHOLD-001` — panic-threshold runtime state

- Outcome: optimized (1 file; two cross-module follow-ups separately tracked)
- Files reviewed: `backends/panic_threshold.rs`, the selector decision path, EdgionGatewayConfig aggregation, EdgionBackendTrafficPolicy publication, panic-mode metrics, and global-state tests
- Changes: removed the unused public `PanicThresholdStore::remove_service`, whose behavior duplicated the canonical `set_service_threshold(service_key, None)` path. Retained the `AtomicU8` global default, per-Service override semantics, validated disable behavior, and ceiling floor calculation.
- Generality and architecture review: independent review approved the deletion. Request selection still clones the global store Arc and enters a `DashMap` shard while threshold, selector, and health/outlier state publish independently; panic threshold was therefore added to `tasks/todo/backend-traffic-policy-atomic-service-snapshot.md` rather than partially cached. The audit also found that `edgion_lb_panic_mode{service}` can remain at one after threshold disable, health/outlier removal, an empty selector, or Service deletion, and that global-state tests lack shared RAII isolation; the lifecycle-wide fix is recorded in `tasks/todo/lb-panic-mode-metric-lifecycle.md`.
- Verification: panic-threshold tests 3/3; selector panic tests 3/3; `cargo fmt --all`; workspace check; workspace clippy (exit 0, 929 pre-existing gateway warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
- Commit: `cd048aa9`

### `BACKENDS-VALIDATION-001` — DirectEndpoint route membership validation

- Outcome: reviewed-no-change (1 file; complex cross-consumer fix included in the existing backend-port task)
- Files reviewed: `backends/validation.rs`, its DirectEndpoint caller, EndpointSlice store reads, route backend filtering, and the final shared unsafe-address validator
- Review: retained denied-reference filtering, typed Service backend selection, fail-close absence behavior, and the final canonical loopback/link-local guard. Independent review confirmed that the fallback path incorrectly compares a `backendRef.port` Service port directly with an EndpointSlice serving port, so a valid `443 -> 8443` mapping is rejected; an explicit endpoint port can instead match an unrelated sibling port on the same multi-port slice without proving that the selected backendRef authorized it. The request path also takes two blocking EndpointSlice read locks, deep-clones every slice, scans all endpoints, truncates `i32` with `as u16`, and compares IPv6 addresses textually. A safe fix requires one immutable route-membership and port-resolution proof shared with every protocol consumer, so no isolated cast, allocation, or string-comparison patch was made. DirectEndpoint requirements, numeric address matching, ready/unready semantics, and endpoint-derived Unix-address rejection are now recorded in `tasks/todo/backend-port-resolution-precompiled-fail-closed.md`.
- Verification: existing endpoint validation tests 2/2; two independent static reviews approved the no-change disposition and TODO scope. No runtime code changed in this batch.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`

### `BACKENDS-SERVICE-KIND-001` — canonical backend kind export

- Outcome: reviewed-no-change (1 file)
- Files reviewed: `backends/service_kind.rs`, the resources-layer `ServiceBackendKind` definition, and its direct Gateway consumers
- Review: retained the narrow re-export of the single canonical resources-layer type. Known kinds use exact case-sensitive matching without allocation, omitted or empty kind follows the Gateway API Service default, and unknown values preserve their spelling in `Unsupported` so request and outbound callers fail closed. No duplicate parser, semantic drift, sensitive logging, or meaningful hot-path cost was found.
- Verification: static source and caller review; independent review approved the no-change disposition. No runtime code changed in this batch.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`

### `BACKENDS-PRELOAD-001` — route-aligned load-balancer preload

- Outcome: optimized (1 file; one cross-protocol security fix separately tracked)
- Files reviewed: `backends/preload.rs`, its startup caller, route effective-rule and invalid-reference fields, the shared core-Service classifier, selector-cell hydration, and TCP, UDP, TLS, HTTP, and gRPC backend selection paths
- Changes: excluded denied and unsupported/non-core Service references from startup preload across every route protocol, reusing the shared L4 Service classifier instead of duplicating group/kind rules. `warm_cell` now returns the atomic `hydrate_if_empty` result and emits its success trace only when the cell is actually non-empty, so a concurrent EndpointSlice removal is no longer counted as a successful warm-up.
- Generality and architecture review: preload remains a bounded, startup-only performance hint after initial resource readiness; effective delegated rules, HashSet deduplication, cell-level TOCTOU protection, and count-only info logging are unchanged. Static caller review found that TCPRoute, UDPRoute, and both TLSRoute proxy modes do not enforce a selected backend's Controller-resolved `ref_denied` marker before Service lookup or dialing. That authorization fix crosses weighted invalid-backend semantics, four connection paths, and hot-reload generations, so it was not partially implemented; the complete work is recorded in `tasks/todo/l4-backend-ref-denied-fail-closed.md`. Preload filtering is explicitly not treated as an authorization boundary.
- Verification: gateway library compiled through the focused preload filter (0 matching tests, 3,357 filtered); shared core-Service classifier test 1/1; `cargo fmt --all`; workspace check; workspace Clippy; SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links. Independent static review approved the final diff and deferred-task scope.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`

### `BACKENDS-SELECTOR-001` — allocation-reduced backend selection

- Outcome: optimized (1 primary file plus narrow call-site updates; complex runtime-state work separately tracked)
- Files reviewed: `backends/selector.rs`, its HTTP/gRPC/Service-kind callers, selector-cell API, health and outlier stores, panic-threshold state, consistent-hash consumers, canonical URI accessors, final port/SSRF guards, and selection metrics
- Changes: consistent-hash Header, Cookie, Arg, and SourceIp keys now remain borrowed byte slices for synchronous selection instead of allocating a `Vec<u8>` per request; only the integration metrics-test path materializes its retained String. Query extraction now enters through `EdgionHttpContext::query` rather than reading Pingora's URI directly, while preserving current-query, raw-value, percent-decoded-name, and empty-key-to-round-robin semantics. Each candidate computes its backend hash once and uses it for both active-health and outlier checks; the redundant health `has_service` snapshot read was removed because `is_healthy` already defaults unknown entries to healthy. One health and outlier store handle is captured per selection and reused by panic counting and candidate filtering rather than cloning the global outlier handle per backend.
- Generality and architecture review: independent review approved the borrowed lifetimes, canonical query source, store-handle capture, lazy recovery side effects, and metrics-test ownership boundary. Historical per-policy not-found codes, panic bypass behavior, rejection/recovery accounting, selector-cell hydration, and final shared unsafe-address validation are unchanged. The existing `tasks/todo/lb-panic-mode-metric-lifecycle.md` covers stale one-valued gauges and global-test RAII; `tasks/todo/backend-port-resolution-precompiled-fail-closed.md` covers unresolved port fallback. Eliminating the remaining per-candidate Service-map lookups crosses independently updated health, outlier, panic, and metric state, so it was not partially implemented and is recorded in `tasks/todo/lb-selection-service-runtime-state-capture.md`.
- Verification: selector panic/health tests 3/3 after each API step; `cargo fmt --all`; workspace check; workspace Clippy; SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links. Static review covered all direct `extract_hash_key`, health-predicate, and selector callers.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`

### `BACKENDS-RUNTIME-001` — proof-carrying backend identity and safe Debug output

- Outcome: optimized (1 file)
- Files reviewed: `backends/runtime.rs`, compiled HTTP backend construction, HTTP and AI selection call sites, policy target lookup, credential state, BackendTLSPolicy resolved-secret fields, and the request-stable generation tests
- Changes: replaced `SelectedServiceBackend`'s derived `Debug` with a bounded manual implementation that prints the logical backend proof and only whether a BackendTLSPolicy is present. The old derived path recursively formatted the full policy, including Controller-resolved CA and client-certificate Kubernetes Secrets; serde redaction does not protect Rust `Debug`. Added a sentinel regression through `SelectedHttpBackend::Service` proving resolved Secret fields and values are absent.
- Generality and architecture review: immutable `Arc<Vec<CompiledBackend>>` generation pinning, constructor-validated slot/index coherence, allocation-free Service selection, typed Service/EdgionBackend authorization gates, request-stable TLS-policy capture, and plugin-runtime Arc identity are unchanged. `SelectedAiBackend` already used the same presence-only policy rendering and excludes credential aliases and runtime backend contents. Resource identities in logical Debug contain bounded route metadata, not Secret values.
- Verification: backend runtime tests 9/9, including the new resolved-Secret Debug regression; adjacent selector tests 3/3; `cargo fmt --all`; workspace check; workspace Clippy (exit 0, 929 pre-existing gateway warnings); SSA, metrics inventory, Pingora source, and gateway no-tracing guards. Agent-doc validation retains only its pre-existing 13 missing external Pingora review links. Independent review identified the leak and approved the bounded replacement.
- Integration-test guard: baseline `edgion-tests` tree unchanged at `ae68dedd863020ddd6d035d7404570716429ecd1`
