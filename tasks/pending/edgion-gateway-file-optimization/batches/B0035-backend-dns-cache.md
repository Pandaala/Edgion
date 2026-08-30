# B0035 — Backend DNS Cache

## Scope

- Baseline: `6b3b679b204385c341e4fa55e356a8237ed2bd47`
- Files: `edgion-gateway/src/backends/discovery/dns_cache.rs`
- Allowed support files: colocated unit tests in the target file only
- Forbidden files: every existing `edgion-tests/**` path, unsupported Gateway rustls code, unrelated modules, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`
- Exact commit allow-list: the target file; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; any analyst-verified new deferred record; the B0034 and B0035 batch records

## Context and value gate

- Analysis subagent: `b0035_analysis`
- Coverage summary: target file, sole backend runtime consumer, Service store and conf-handler lifecycle, shared `ResolverSpec`, resolver precedence, EGC TTL, Gateway/Service annotations, LinkSys resolution, target unit tests, frozen DNS integration scenarios, and DNS review precedent
- Key responsibility/call chain: every cached ExternalName request chooses an effective resolver, keys the process-global cache, resolves on miss, and stores addresses for the configured TTL
- Concrete opportunity and decisive evidence: the cache key omits the mutable queried hostname, while Service updates and removals do not invalidate entries; a renamed ExternalName can reuse old addresses until TTL and deleted unique Service keys can remain resident indefinitely
- Expected benefit: complete query identity plus explicit invalidation/bounding would prevent stale-host reuse and process-lifetime cache growth
- Regression surface: requires a choice among full query keying, Service-generation invalidation, bounded-cache adoption, or sweeping, with caller and lifecycle changes outside the target
- Decision: defer

## Implementation

- Implementation subagent: none; the verified issue exceeds the frozen batch boundary
- Complexity: complex
- Plan summary: not applicable; no implementation is authorized
- Actual change: none
- Behavior intentionally preserved: documented controller/gateway/per-request mode selection, resolver precedence, TTL, and failure semantics remain unchanged
- Tests added or updated: none

## Independent review

- Reviewer: `b0035_defer_review`
- Verdict: LGTM
- Must Fix resolution: none
- Deferred observations: D0001 correctly combines the incomplete query identity, missing Service lifecycle invalidation, and unbounded deleted-key retention; a target-only allocation cleanup was rejected because it would entrench the flawed key contract
- Adjudication: none

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Runtime target diff | passed | `dns_cache.rs` remains unchanged |
| Format | passed | `cargo fmt --all`; `cargo fmt --all -- --check` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace lint | passed | `cargo clippy --workspace --all-targets`; existing warnings only |
| Repository guards | partial | SSA, metrics inventory, Pingora source, and Gateway no-tracing passed; independent baseline diagnosis and one retry confirmed the exact 13 pre-existing missing external Pingora links |
| Targeted tests | not applicable | Record-only defer; no runtime code changed |
| Frozen integration paths after review and verification | passed | 2,681 existing paths matched the manifest |

## Handoff

- Ledger/changelog updated: yes
- Deferred finding: [`D0001`](../deferred/D0001-external-name-dns-cache-identity-lifecycle.md)
- Commit: `16d39e25`
