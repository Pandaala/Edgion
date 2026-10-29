# R0002 - Supply-chain prerequisite repair

## Scope and ownership

Original repository-pruning goal continues. D0009 blocks the required dependency-security gate; B0001 must be suspended without losing its independently reviewed patch. User requested continuation after the blocker report; implement only reviewed prerequisite repairs. Protected user impl/ and original pruning specification remain untouched. No push or new advisory ignore.

## Stage 1 plan (independently approved)

1. Save and verify exact B0001 Cargo.lock/Gateway manifest diff against HEAD 6407efe23012ad6f53622f7ad53c33c662c1accb, then restore only these campaign-owned changes. B0001 becomes queued; no simultaneous mutation lane.
2. Freeze the post-suspension baseline, manifests/config, source revisions and supported resolution features. Repair edits are restricted to Cargo.lock.
3. Precisely update pulldown-cmark 0.13.5, rustls 0.23.46, rustls-webpki 0.103.16, tokio 1.53.3, webpki-roots and webpki-root-certs 1.0.10, wasmtime 49.0.4, and zstd-sys 2.1.2+zstd.1.5.7. Permit only independently verified required existing Wasmtime/Pulley/internal 49.0.4 and Cranelift 0.136.4 coupled packages. Stop for unexpected identities, versions, features, sources, removals or MSRV. No unrestricted update.
4. Prove exact package/source/feature containment and scan actual advisory identities. Expected partial result: 24 removed, four remaining (RUSTSEC-2026-0391, RUSTSEC-2026-0398, RUSTSEC-2026-0403, RUSTSEC-2024-0449). Counts are predictions until measured.
5. Independently review diff and run required workspace checks, full tests, default integration and affected supported allocator builds. Preserve all existing tests and resource/security invariants; no Gateway rustls/all-features. No commit while the required supply-chain gate remains unresolved.

## Remaining constraints

Canonical Pingora header-serde still constrains zstd 0.13/safe 7; Wasmtime cache retains zstd 0.13. YAML maintenance remediation must cover workspace, Pingora and Kube consumers; serde_yaml_ng still depends on unsafe-libyaml, whereas yaml_serde/libyaml-rs is a candidate requiring separate semantic review. No automatic exemption or local third-party rewrite is permitted. Independently plan later phases after concrete Stage 1 results.

Plan review Approved, no Must Fix: reviewer independently verified all eight root targets and the complete coupled closure exist, are not yanked, and fit Rust 1.96.1. Implementation authorized in the sole lane, including exact B0001 suspension proof. Unexpected package/feature/source changes still require renewed review. Full supply-chain success and commit remain blocked until residual advisories are addressed.

B0001 suspension completed by the implementer: exact patch, original files, HEAD and full hashes saved under /private/tmp/edgion-r0002-stage1/ (b0001.patch, snapshot.json, b0001-* and baseline-*). Both owned files were verified equal to HEAD after restoration. R0002 is the sole mutation lane; B0001 is held for fresh reapplication and analysis after the prerequisite is resolved.

Persistent B0001 patch: `B0001-suspended.patch`. Snapshot hashes: `Cargo.lock` = `3430ff0ebe7b9387b6c16f33551437f7620061829c8ae81e4fb9577ca4effa94`, `edgion-gateway/Cargo.toml` = `ac12d906054a871ce2d9e84afe29601dd9608ccdbbdeef30e29d369847b92221`.

## Reviewed resolver drift correction

After precise pulldown/rustls updates, containment caught Cargo rewiring unchanged tempfile 3.27.0 from getrandom 0.4.3 to 0.3.4. Implementation stopped. Independent reviewer verified original constraint >=0.3,<0.5 and approved restoring exactly the original edge, followed by full resolving metadata --locked and feature-tree acceptance with unchanged lock hash. No scope expansion; subsequent unrelated drift still stops. Evidence: /private/tmp/edgion-r0002-stage1/update-rustls.log.

Tokio update exposed resolver rebindings in 13 unchanged blocks (14 edges), including original socket2/windows-sys choices. Independent reviewer verified every original constraint and current metadata stability, then approved a guarded dependency-array restoration: match name/version/source, reject checksum or non-dependency drift first, restore only unchanged-package dependency arrays with uniquely resolving approved version token substitutions. Upgraded blocks are never auto-restored. Full locked resolution and 13 feature comparisons remain mandatory; different graph drift still stops.

Wasmtime precise resolution introduced ahash 0.7.8 by rebinding unchanged pingora-lru from hashbrown 0.17.1 to 0.12.3. Independent review rejected the initially misstated hashbrown-to-ahash restoration: baseline hashbrown 0.12.3 has no dependencies, and its optional ahash constraint is ^0.7, not ^0.8. Approved exact correction restores pingora-lru's valid original hashbrown 0.17.1 edge, restores hashbrown 0.12.3's baseline no-dependency block, removes only the verified orphan newly added ahash 0.7.8 and canonicalizes now-unique remaining references. Locked parsing, source/checksum guards and final feature comparisons remain required.

## Stage 1 implementation frozen

Only Cargo.lock changed: 35 approved package substitutions, 76 insertions/76 deletions. All 13 normalized feature sets match frozen baseline; manifests/deny.toml and Git package records remain unchanged. Final lock SHA256: 70837149aad739539cfeb111a270353f8c8418329f1b6fe7b576f8b52ba39db5. Evidence directory /private/tmp/edgion-r0002-stage1/ contains containment.json, feature-proof.json, final.patch and scans.

Fresh cargo deny still exits 1 but now reports only the expected four advisory errors; bans/licenses/sources pass. Raw cargo audit also exits 1 and reports additional hickory/Quinn findings and warnings. Their difference from deny selection is under independent investigation, not an accepted exemption. Independent final review and locked workspace compile check are running; no commit or full-gate success claim.

Independent raw-audit investigation verified five optional inactive quinn-proto baseline advisories and ten additional zstd-safe unsound warnings excluded by deny's default transitive scope. Added all to D0009. Narrow ninth-root patch quinn-proto 0.11.16 to 0.11.18 independently approved (MSRV1.85, quinn constraint^0.11.15, exact registry checksum verified); implementation waits for current compile check completion to avoid mutating its lockfile. No HTTP/3 activation, manifest or policy change. Strict/raw audit evidence remains mandatory.

## Final ninth-root state

The approved quinn-proto patch is complete: 36 package substitutions total, only Cargo.lock (78 insertions/78 deletions), final SHA256 ce260fb93b56a345339cfc4b0f3cfc9897d64e8c58c8bea0f171bd27f190bfb1. All 13 normalized feature comparisons pass. All five Quinn advisory IDs are absent from raw and strict audit. Remaining unresolved inventory is three Zstd vulnerability advisories, ten zstd-safe unsound warnings and unsafe-libyaml maintenance; four other raw records retain existing explicit policy, not new exceptions. cargo deny retains its four expected errors; raw and strict audit still fail.

The original 35-package state passed locked workspace all-target check and independent final review. Final Quinn addendum review, latest Clippy and full workspace tests are running; source remains frozen. No commit permission or full supply gate success.

## Runtime verification progress

Final 36-package SHA passed independent final review. Latest locked Clippy passed with existing warnings; full locked workspace test sweep exited 0 (78 successful summaries, 13,152 passed, zero failed, 67 existing ignored). Default, system and mimalloc standalone Gateway all-target checks each passed. Static format/docs/SSA/metrics/Pingora/transform/unit-layout/tracing guards passed. Canonical integration preparation is running; default integration remains pending. Source is frozen; security gate still fails on the explicitly recorded residual inventory.

## Default integration environment failure

Current default integration exposed WebSocket conn_est and four L4 idle-close log-probe timeouts. Independent read-only diagnosis found LocalFile disk_ok=false from initialization; access/tcp/tls files remained empty. Production statvfs usage integer is 90 and the unchanged default requires <90. Protocol echo/reverse-progress pass; no evidence attributes these failures to upgraded Tokio. Existing health contract intentionally sheds file output at this threshold.

Current worktree Rust incremental cache occupies 310683676 KiB (about296GiB); build processes are not active. After the current suite ends, verify the cache path is workspace-local/generated and no compiler is active, remove only rebuildable incremental cache, retain binaries/logs/user artifacts, confirm adequate space and rerun both failed suites then the unchanged default gate. Do not change production thresholds, test assertions or deadlines. Current run cannot pass without file logs.

## External prerequisite revalidation

Independent read-only adjudication rechecked canonical Pingora remote head 7708876aec0a3593b7b53970c6ee7899b8c34de8: header-serde still requires zstd 0.13.1 / zstd-safe 7.1.0; core still requires serde_yaml 0.9. The nonoptional proxy -> cache -> header-serde chain survives disabling application/Wasmtime caches. No compatible lock update, downgrade, local YAML migration or current remote advance can remove the full remaining inventory. Canonical fork maintainers must review and publish fixed-major Zstd adaptations and a maintained YAML replacement before this workspace can consume the result. Vendor rewriting, alternate sources, new ignores and push are outside authorization. Further local consumer migrations remain necessary after that publication. This is an external security-gate blocker, not a runtime-test failure.

## Environment repair and targeted revalidation

The first default run exited 1 with exactly the two diagnosed log-dependent suites failing; final residue/metrics/requeue guards passed. Confirmed target/debug/incremental was a local non-symlink ignored generated directory with no active cargo/rustc, then removed only that rebuildable cache. Disk statvfs usage fell from 90 to 80 percent, with 372 GiB available; binaries, test logs and protected files were retained. Unchanged WebSocket and DuplexBackpressure targeted runners both exited 0. Evidence: /private/tmp/edgion-r0002-websocket-retest.log and edgion-r0002-l4-retest.log; reports testing_20261029_132246 and testing_20261029_132331. Full unchanged default rerun is active in /private/tmp/edgion-r0002-integration-retest.log. No code/test/threshold changes were needed.

## Final runtime result and handoff

The unchanged complete default rerun exited 0: 290 report groups, 279 passed, zero failed/timeouts, 11 existing default skips. Final metrics, unknown-field, requeue and cache-residue guards passed; owned services were cleaned up. Report: tmp/integration_testing/testing_20261029_132708/report.log; full log: /private/tmp/edgion-r0002-integration-retest.log. All required runtime/build validation for the reviewed partial patch is successful; security scans still fail on the independently recorded residual inventory.

Campaign remains blocked by the external canonical Pingora dependency prerequisite. Keep the reviewed Cargo.lock patch uncommitted and B0001 suspended; do not promote B0002/B0003 or claim goal completion. No push, new advisory ignores or third-party edits occurred. Revalidate upstream availability, current HEAD and exact graph before resuming.

Independent final addendum accepted the partial repair with no Must Fix: verified final default report, exact frozen lock hash/diff and prior build/test/feature evidence. Required security gate remains failing; no commit or completion acceptance. Final record-only agent-doc validation and diff whitespace check passed.

## Stage 2 local prerequisite assessment

Fresh independent analysis confirmed useful local work before upstream publication: migrate the Common List/Watch codec to fixed zstd 0.14.3 while retaining default-features=false. Initial implementation scope is Cargo.toml and Cargo.lock only. Plan-first review must prove one unchanged zstd-sys 2.1.2 instance, preserved frame/error/resource semantics and intentional supported-feature graph differences. Source changes require renewed scope review. This removes the active Common old-version path but cannot remove remaining advisory IDs while Pingora retains old versions.

A later separately reviewed phase may remove only Wasmtime's unused disk-cache default feature while retaining all other current defaults. YAML migration remains deferred pending an integrated consumer design. Both phases stay within R0002's single serial lane; no next pruning batch or security waiver.

Stage 2 implementation plan is awaiting independent review: freeze Stage 1; change only root manifest and lock to add zstd 0.14.3 / safe 8.2.0 and move Common's edge; prove exact single unchanged sys and package/feature containment across 13 scenarios; run four existing codec/transport targets plus temporary bidirectional old/new standard-frame checks; then run required checks and scans and obtain final independent review. Preserve all production bounds, error and cancellation contracts. Unexpected API/source/test/sys/feature changes stop for renewed review.

Stage 2 plan independently approved with no Must Fix. Execution authorized within Cargo.toml/Cargo.lock. Version-aware graph comparisons and retained temporary interoperability source/lock/results are mandatory; no source/test edits. Implementation owns serial targeted validation; coordinator runs broader gates only after freeze.

Stage 2 implementation frozen at SHA256 30e0a05f76410135a87d6880c5767dbe696d454032218117940afa8864b42267. Only two approved package additions and Common edge migration; full sys record and other sources/edges unchanged after guarded resolver restoration. All 13 version-aware feature checks passed, Common 7 tests and three real transport tests passed, retained temporary interoperability source/lock verified 98 checks and a locked rerun. An initial temporary harness typo was corrected without repository changes; original failed log retained. Evidence: /private/tmp/edgion-r0002-stage2/. Independent final runtime addendum is pending. Stage 3 Wasmtime cache-removal plan is being prepared without edits; proposed combined final workspace gates require review and remain mandatory.

Stage 2 final targeted review LGTM, no Must Fix; reviewer approved combining broad checks/scans after Stage 3 in the same R0002, with no commit/batch promotion and final integration still mandatory.

Stage 3 plan awaits independent approval: retain exact Wasmtime 49.0.4 defaults except cache using explicit features; permit only the 11 proved cache-exclusive package removals; preserve stage 1/2, Git/sys identities and unrelated edges. Version-aware feature proofs must establish that every removed transitive feature is cache-exclusive (host rustix/process identified); any unexplained delta stops. Existing Wasm unit and real streaming transport tests plus supported allocator checks precede final combined gates. Source/tests and private runtime/pool/OCI cache ownership remain unchanged.

Stage 3 plan approved after correcting the real transport command to include --features test-util. All three named wasm_stream_transport tests must execute and pass; zero-test success is rejected. Implementation authorized within the exact two-file/11-removal scope.

## Stage 3 implementation and combined verification

Frozen lock SHA256 edec257534ba5369737089fe9c196e932af5ddc5625ed13bc5c2c4f33c36acf9. Wasmtime explicitly retains 25 original defaults except cache; exactly 11 approved cache-exclusive packages removed. All surviving package identities/checksums and complete sys/Git records remain unchanged. All 13 graphs passed the approved cache-only feature delta, including rustix/process; Common graph unchanged. Wasm unit tests passed 281/281; explicitly enabled test-util real stream transport passed all three required named tests. Evidence: /private/tmp/edgion-r0002-stage3/.

Format and static docs/SSA/metrics/Pingora/transform/unit-layout/tracing guards passed. Fresh deny/raw/strict scans still exit 1; independent comparison found exactly the same advisory IDs and unchanged policy. Old Zstd consumers now consist only of Pingora core/header-serde; Common and Wasmtime no longer use the vulnerable old pair. YAML remains across workspace, Kube and Pingora. Fourteen unresolved entries remain; no waiver or ID-count reduction claim. Combined workspace/allocator checks and final integration are in progress.

Stage 3 final targeted review LGTM, no Must Fix. Combined locked workspace all-target check and Clippy passed; default/system/mimalloc standalone Gateway all-target checks passed. Static guards passed. Canonical integration preparation is now rebuilding final-state binaries; complete default integration remains pending. Build evidence: stage3/build-results.json and per-command logs.

## Final integration synchronization failure under diagnosis

The final run testing_20261029_161616 exposed Dyn_ResponseCacheDynamic target-phase origin convergence failure. Read-only diagnosis found wire/access-log 503 with BackendEndpointSliceNotFoundByRoundRobinDefault while EndpointSlice and HTTPRoute generations propagated separately; the test waits only unchanged plugin state before its first target request. Other scenarios continue. Initial evidence points to an existing narrow test synchronization defect, not a compression/runtime failure.

A plan-only repair assessment is authorized for edgion-tests/src/client/suites/edgion_plugins/response_cache/mod.rs under the user's existing baseline-repair scope. Preserve all cache assertions, original deadline/poll interval and failure semantics; only a proved target-convergence window may wait. No edit before independent plan approval and completion of the current run.

The complete run exited 1 after cleanup with only Dyn_ResponseCacheDynamic failing. Independent diagnosis and plan review agree on the existing synchronization defect. One-file repair plan approved: before status failure, only target/no-valid-receipt/wire-and-log503/exact endpoint-missing diagnostic/unchanged origin counter may clear receipt and continue the existing bounded loop. Once a valid target miss exists, the same error must fail. Add pure negative cases and require later actual miss then matching hit. No production behavior, deadlines, bounds or assertions change. Implementation authorized after runner cleanup; dependency hash remains frozen.

## ResponseCache synchronization repair implemented

One-file repair adds a narrow endpoint-convergence predicate and one negative/receipt-sequence regression test, preserving existing tests and runtime assertions. Independent code review provisionally LGTM; production behavior and dependency hashes unchanged. Initial --bin test_client filter ran zero tests and is explicitly invalid evidence; independent review corrected the target to --lib response_cache::tests, which passed all seven tests. Client build passed.

Canonical targeted ResponseCache runs passed twice (testing_20261029_163500 and testing_20261029_163557), including original static and complete dynamic scenarios and final guards. Required final checks are rerunning before full default integration. Evidence: /private/tmp/edgion-r0002-response-cache-sync/ (fix.patch, unit-lib.log, dependency-hashes.json, targeted logs, checks.json).

After the one-file repair, all final quick checks passed. The first full rerun stopped during isolated preflight because only test_client had been rebuilt: test_server predated the changed integration source. Independent diagnosis classified a coordinator build-preparation omission, not a functional regression; no scenario services had started for that failing guard. This run is incomplete, not passing. Re-run the canonical prepare.sh for all binaries/extra targets, then the unchanged complete default runner; do not bypass freshness checks.

## Final prepared default acceptance

Canonical preparation after the test-source fix exited 0. Complete default integration then exited 0 with 290 report groups: 279 passed, zero failed/timeouts, 11 existing default skips. Report: tmp/integration_testing/testing_20261029_164544/report.log; full log: /private/tmp/edgion-r0002-response-cache-sync/integration-prepared.log. ResponseCache dynamic passed within this full run; final metrics/config/requeue/cache-residue guards passed and owned services cleaned up. Final format/workspace check/Clippy/static guards and diff whitespace check passed. Final independent acceptance addendum remains pending.

No remaining runtime baseline failure is known from these gates. Supply scans still fail on the previously adjudicated 14 entries; upstream canonical Pingora dependency publication and the broader YAML migration remain required. R0002 stays uncommitted and the original source-pruning goal remains incomplete; no next batch promotion or push.

Final independent acceptance: LGTM, no Must Fix. Reviewer verified seven real module tests, both targeted runs, prepared full default report, all final quick checks, exact source patch and frozen dependency hashes. Partial repair is verified; unresolved security gate still prevents commit and goal completion.

## User-authorized local closeout

User explicitly instructed: defer upstream issues and continue repository-owned code. This supersedes the earlier campaign stop/commit gate for the known 14 upstream dependency records only. Keep scans failing and D0009 open; do not add ignores or represent security acceptance. All local changes already passed independent review and applicable checks. Frozen implementation hashes are unchanged. Commit this verified local repair and resume B0001; upstream remediation is no longer the active mutation lane.
