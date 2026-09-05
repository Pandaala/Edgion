# B0062 — HTTP Plugin Snapshot and Delta Semantics

## Scope
- Baseline: `695f0ba999d71b257a66a653d34fd608a7a75775`
- Pipeline state: reviewed and fully verified; pending local commit
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/{conf_handler_impl.rs,mod.rs,plugin_store.rs}`
- Exact implementation files: `edgion-gateway/src/plugins/http/{conf_handler_impl.rs,plugin_store.rs}`
- Supporting files: `edgion-gateway/src/plugins/runtime/session_adapter.rs` for exactly two unit-test call-site migrations; `edgion-gateway/src/observe/access_log/log_ext.rs` for exactly two stale AtomicBool/Relaxed comment corrections
- Expected no-change file: `edgion-gateway/src/plugins/http/mod.rs`
- Allowed tests: colocated tests in the two implementation files only
- Forbidden files: every existing `edgion-tests/**` path, config cache and handler traits, Controller/resources/schema code, metrics/docs, other plugins, and protected user paths; no production behavior change is allowed in the two supporting files
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact future commit allow-list: the two implementation files; the two narrowly supporting files above; campaign ledger/changelog/main record; B0061 and B0062 batch records; reviewer-approved identity and integration-coverage task records plus campaign deferred backlinks; `deferred/INDEX.md`

## Context and value gate
- Analysis subagent: `b0056_implementation`, revalidated against `695f0ba9`; all target blobs and relevant callers remain unchanged from the queued report.
- Baseline: conf-handler tests pass 8/8 and plugin-store tests pass 12/12.
- Publication bug: `PluginStore` publishes its immutable plugin map and the derived `has_access_log_extern` flag separately. Removing the last declaration can expose `false` with an older map that still contains live declarations, an impossible mixed generation that can omit configured canonical access-log fields.
- Delta bug: the partial config handler merges add and update before calling the store. A delete-to-create-coalesced same-key add with invalid DSL or WAF can therefore retain and serve the deleted previous runtime through update-only last-known-good policy.
- Decision boundary: publish the map and its derived flag in one immutable snapshot; keep compiled adds and updates distinct so adds replace unconditionally while updates retain the existing DSL/WAF last-known-good behavior.
- Expected no-change file: `plugins/http/mod.rs` retains a minimal module inventory and live public exports.
- Preserved behavior: one serialized config writer, full-snapshot absent-key deletion and current name-keyed LKG behavior, update-only DSL/WAF retention, fail-closed non-retainable failures, adaptive reconciliation after publication, count-only config logs, plugin stages, access-log redaction/blocklists, and a lock-free request path.

## Deferred candidates
- Full snapshots cannot distinguish an invalid same-object update from a delete/recreate with the same namespace/name. Correcting this requires a cross-source identity contract analogous to the TLS last-known-good identity task, not a plugin-local UID shortcut.
- Existing integration tests remain frozen. A separate future scenario should cover invalid update retention, invalid delete/recreate replacement, reconnect parity, and valid recovery after the identity design lands.

## Plan-first implementation
- Implementer: `b0055_runtime_analysis`
- Complexity: complex; changes the hot-path shared-state carrier and partial-config LKG semantics together
- Required plan review: approved independently by `b0056_final_review` after corrections
- Approved plan: replace the map and AtomicBool with one private immutable ArcSwap snapshot; publish once per full or delta mutation; implement one `pub(crate)` delta API with remove, unconditional add, then update-only LKG order; keep add/update compilation separate; reconcile adaptive plugins after publication.
- Supporting corrections: migrate exactly two session-adapter unit-test calls to the unambiguous delta API and update exactly two access-log comments from a Relaxed atomic load to a lock-free snapshot load. No compatibility wrapper is allowed.
- Test contract: prove original compiled Arc retention on invalid update, unconditional nonactivatable replacement on invalid same-key add, deterministic flag transitions, and bounded same-guard concurrent snapshot coherence without scheduling assumptions or barrier deadlock.
- Actual change: one immutable ArcSwap snapshot now owns the plugin map and derived AccessLogExtern gate; the unique delta API removes, installs adds unconditionally, applies update-only LKG, and publishes once before adaptive reconciliation. Two session-adapter unit-test calls use the explicit delta API, and two access-log comments describe the new lock-free snapshot load.

## Independent review
- Plan reviewer: `b0056_final_review`
- Code reviewer: `b0056_final_review`
- Plan verdict: Approved after removing the ambiguous test-only API, narrowing two supporting files, and correcting the snapshot-coherence test design
- Code verdict: Approved with no Must or Should findings; the reviewer confirmed the single-writer call chain, one-publication boundary, original/incoming Arc assertions, same-guard coherence, bounded stress, and exact supporting-file narrowness.
- Deferred adjudication: D0030 records the nonduplicate cross-source EdgionPlugins identity design and the later source-to-reconnect scenario. D0031 is closed as not a standalone defect; see `skills/04-review/testing/edgionplugins-lkg-identity-coverage-not-standalone.md`. Existing integration tests remain frozen.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | user-authorized concurrent baseline unchanged |
| Baseline conf-handler tests | passed | 8 tests |
| Baseline plugin-store tests | passed | 12 tests |
| Implementation and focused tests | passed | 9 conf-handler, 14 plugin-store, and 107 session-adapter tests; coherence stress also passed 10 repeated runs in review |
| Gateway all-target compile | passed | implementation verification |
| Independent code review | passed | no Must or Should findings; format and diff checks passed |
| Full campaign gates | passed | format, workspace check, workspace Clippy, agent docs, SSA, metrics inventory, Pingora source, and gateway no-tracing |
| Final frozen integration paths | passed | existing integration tests unchanged from the user-authorized baseline |

## Handoff
- Ledger/changelog updated: yes; two changed and one examined-no-change
- Commit: `19544491`
