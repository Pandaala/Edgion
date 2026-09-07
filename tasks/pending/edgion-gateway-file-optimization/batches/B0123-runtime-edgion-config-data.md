# B0123 — Runtime EdgionConfigData

## Scope

- Baseline: `486e8fea12bd870325b489839262729d14ecb22c`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs`
  - `edgion-gateway/src/plugins/runtime/edgion_config_data/mod.rs`
- Baseline blobs: `74efcd1688c1eb8737f603300951dac63ab75902` and `8fbd5f08e47aa48fb5e8d516283b30dbbeeea6e3`
- Exact implementation allow-list:
  - `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs`
  - `edgion-gateway/src/plugins/runtime/edgion_config_data/mod.rs`
  - `edgion-gateway/src/plugins/http/ip_restriction/plugin.rs`
  - `edgion-gateway/src/plugins/stream/ip_restriction/stream_ip_restriction.rs`
  - `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs`
  - `edgion-gateway/src/plugins/stream/global_connection_ip_restriction/plugin.rs`
  - `skills/04-review/plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md`
- Forbidden files: every file outside an independently justified exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary

- Read the complete module, config-sync handler/registration, immutable publication and lookup consumers, resource schema/validation, hot-reload/delete behavior, tests, and relevant review knowledge.
- Preserve generation consistency, secret/redaction rules, canonical KeyGet/template ownership, bounded lookup cost, binary-owned operational logging, canonical access-log ownership, and observability restraint.
- Existing integration tests remain frozen. Create a separate issue only if genuinely necessary new integration coverage cannot be represented without editing an existing file.

## Handoff

- Analysis: change. Separate raw and matcher lookups currently load the global ArcSwap independently for every key and again for deny/allow groups; GCIR also resolves its Selector before separately loading referenced IpLists. One atomic multi-key update can therefore be observed as a Selector/IpList or multi-IpList combination that never existed, including a transient deny miss. Capturing one immutable raw-plus-index snapshot per policy decision closes that generation-coherence gap and removes repeated ArcSwap/refcount/heap-vector work on request and connection paths.
- Proposed implementation scope: the two owned module files; the HTTP, stream, TLS-route, and global connection IP-restriction consumers; focused inline tests; and `skills/04-review/plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md`. A narrow Gateway-side architecture paragraph is optional only if independent review proves it necessary; no broad documentation rewrite is authorized.
- Preserved behavior: one atomic publication/writer, full/partial/delete and matcher-reuse semantics, deny-before-allow/default policy, fail-closed unresolved allow refs, partial-good IpLists, canonical IP matching, no new cache/telemetry/failure shape/KeyGet/template/secret handling, and English-only writes.
- Candidate plan: expose an immutable captured snapshot with raw lookup and matcher resolution, update all four restriction consumers to capture once per decision, make GCIR resolve Selector and referenced profiles from the same handle, add old/new-generation coherence tests, and correct the focused regression knowledge. No integration issue is required because the deterministic concurrency boundary belongs in unit tests and existing frozen scenarios already cover config sync/hot reload.
- Complexity: complex. The analyst rechecked the campaign rule and confirmed plan-first is mandatory because the exact scope spans six Rust files plus one knowledge page, four request/connection hot-path consumers, and an immutable-generation concurrency/security boundary. The optional architecture page is excluded from the exact proposed allow-list.
- Freshness: analyzed at `486e8fea`; both target blobs matched their recorded baselines and B0121 worktree changes were not modified.
- Promotion freshness: revalidated at `5a95df60`; all seven frozen paths retain their analyzed blobs. B0121 changed condition publication and B0122 changed records only, leaving the EdgionConfigData store and all four authorization consumers unchanged.
- Duplicate ownership: this batch activates and resolves the existing D0048 finding rather than creating a duplicate task.
- Plan: approved after correction. The implementation will add a crate-private owned snapshot handle, preserve zero global-store loads for inline-only/no-selector-no-ref decisions, resolve every referenced matcher used by one authorization decision through one captured generation, and prove the former fail-open with an explicit torn old-Selector/new-IpList tuple while both complete generations deny.
- Independent plan review: the first pass required the empty-reference fast path and a stronger torn-generation regression. The revised plan also made the handle crate-private, fixed the GCIR borrow/control-flow contract, retained one ordered `SmallVec` per side without an allocation-free claim, and received LGTM at unchanged HEAD `5a95df60`.
- Implementation: added the opaque crate-private owned snapshot and lazy reference helper; migrated HTTP, L4, TLS-route, and global connection restrictions so each referenced authorization decision uses one generation; kept reference-free decisions at zero global-store loads; and added the explicit two-key torn-tuple regression. D0048 is resolved.
- Independent final review: runtime code, ownership, lifetimes, fast paths, publication, ACL behavior, IP identity, GCIR fallback/observability, API migration, deterministic regression, exact scope, and frozen tests were approved. Two documentation-only Must Fix rounds corrected HTTP versus stream temporary-container and spill-threshold wording; final re-review was LGTM with no remaining finding.
- Verification: 18 EdgionConfigData, 12 HTTP IpRestriction, 2 stream IpRestriction, 3 TLS-route IpRestriction, and 12 GCIR focused tests passed. `cargo fmt --all`, `cargo check --locked --workspace --all-targets`, `cargo clippy --locked --workspace --all-targets`, `cargo build --locked -p edgion-gateway --bin edgion-gateway`, agent-doc, SSA force, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and frozen 3,084-file integration guards passed; Clippy emitted only the existing warning set.
- Coverage: both owned runtime files and three previously pending stream support files are changed; the already-terminal HTTP support file was revisited without duplicate credit.
- Integration disposition: no existing integration file changed and no new integration issue is needed; the publication interleaving is deterministically covered at the store boundary while existing frozen scenarios retain ordinary sync/hot-reload coverage.
- Commit: pending.
