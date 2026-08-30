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
