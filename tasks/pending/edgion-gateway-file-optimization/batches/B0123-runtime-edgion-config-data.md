# B0123 — Runtime EdgionConfigData

## Scope

- Baseline: `486e8fea12bd870325b489839262729d14ecb22c`
- Pipeline state: analysis-ready
- Active anchor crate: `edgion-gateway`
- Candidate owned files:
  - `edgion-gateway/src/plugins/runtime/edgion_config_data/conf_handler.rs`
  - `edgion-gateway/src/plugins/runtime/edgion_config_data/mod.rs`
- Baseline blobs: `74efcd1688c1eb8737f603300951dac63ab75902` and `8fbd5f08e47aa48fb5e8d516283b30dbbeeea6e3`
- Candidate implementation allow-list: not frozen until analysis completes
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
- Review: pending.
- Verification: pending.
- Commit: pending.
