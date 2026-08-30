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
