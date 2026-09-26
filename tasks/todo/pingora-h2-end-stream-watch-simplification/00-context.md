# Context: Pingora HTTP/2 END_STREAM Watch Simplification

## Read first

1. [`pingora-h2-end-stream-watch-simplification.md`](pingora-h2-end-stream-watch-simplification.md)
2. `skills/00-design-rules.md`
3. `skills/SKILL.md`
4. `skills/07-tasks/new_feature_work_flow.skill`
5. `skills/05-testing/SKILL.md`
6. `skills/04-review/SKILL.md`

## Required skill paths

- `skills/00-design-rules.md`
- `skills/07-tasks/SKILL.md`
- `skills/07-tasks/new_feature_work_flow.skill`
- `skills/07-tasks/new_feature_work_flow_templates.md`
- `skills/07-tasks/new_feature_work_flow_subagents.md`
- `skills/05-testing/SKILL.md`
- `skills/05-testing/00-unit-testing.md`
- `skills/05-testing/01-integration-testing.md`
- `skills/04-review/SKILL.md`
- `skills/04-review/SKILL.md`
- `skills/09-misc/cicd/02-github-workflow.md`

Load only the directly relevant files beneath these indexes. This task changes the Pingora fork rather than Edgion runtime code, but Edgion's testing, review, English-only, upstream-source, and task-lifecycle rules still apply.

## Review sources

- Stable review ledger: `/Volumes/ExtStore/ws3/pingora-release-0.8-review/docs/edgion-fork/review-release-0.8/REMEDIATION-EDGION-V2.md`, entries #2, #4, and #29.
- Historical review IDs: `p2-b9ba47c-end-stream-watcher-has-an-unenforced-h2-version-contract`, `p1-b9ba47c-invalid-h2-trailers-can-be-laundered-as-clean-eof`, and `p3-h2-invalid-trailer-cache-admission-not-covered`.
- Closed invalid-trailer implementation anchor: Pingora `edgion_v3` commit `682506d`, plus the pre-existing poll-error-latch follow-up recorded in the ledger.

## Pingora source anchors

- `/Volumes/ExtStore/ws3/pingora/Cargo.toml`
  - Inherits upstream/main's `h2 = ">=0.4.16"` declaration.
- `/Volumes/ExtStore/ws3/pingora/.gitignore`
  - Ignores `Cargo.lock`; a local lockfile is not a library-consumer contract.
- `/Volumes/ExtStore/ws3/pingora/pingora-core/src/protocols/http/v2/end_stream_watch.rs`
- Approximately 779 lines of production documentation, IO wrapping, frame
  scanning, and shared evidence state, followed by approximately 1,190 lines
  of tests and benchmarks. Measure these categories separately.
  - Its h2 0.4.15 and tracked-lockfile explanation is stale.
- `/Volumes/ExtStore/ws3/pingora/pingora-core/src/protocols/http/v2/client.rs`
  - `Http2Session` response completeness proofs, peer/local reset bookkeeping, body/trailer async and poll APIs, raw-wire h2 contract tests.
- `/Volumes/ExtStore/ws3/pingora/pingora-core/src/connectors/http/v2.rs`
  - Wraps production connection IO with `EndStreamWatchStream` and carries watch state into sessions.
- `/Volumes/ExtStore/ws3/pingora/pingora-core/src/protocols/http/v2/server.rs`
  - Related downstream END_STREAM/reset behavior that must not regress during shared changes.
- `/Volumes/ExtStore/ws3/pingora/pingora-proxy/tests/test_h2_upstream_no_error_reset.rs`
  - Cross-layer remote-reset and termination coverage.
- `/Volumes/ExtStore/ws3/pingora/pingora-proxy/src/proxy_cache.rs` and related cache tests
  - Cache admission/completion must reject every failed or ambiguous upstream body.
- `/Volumes/ExtStore/ws3/pingora/docs/edgion-fork/h2-end-stream.md`
  - Current evidence model and dependency-upgrade guidance.

## Upstream evidence to establish during analysis

- Pingora commit `b3317e3` raises h2 from `>=0.4.14` to `>=0.4.16`.
- h2 0.4.16 adds reset-after-received-END_STREAM preservation through its stream state.
- The locally resolved h2 0.4.19 includes the same public behavior, but the ignored local lockfile does not constrain library consumers.
- Determine which completion cases are observable through public `RecvStream` and error APIs at both the supported minimum and current/default resolution.
- Treat private h2 source inspection as supporting evidence only; replace private assumptions with executable public-behavior contracts wherever possible.

## Mandatory safety constraints

- Do not modify Cargo manifests before the dependency policy receives explicit design approval.
- Do not downgrade dependencies or restore an older Pingora/h2 implementation.
- Do not delete or bypass production watcher logic during initial analysis; use test-only baseline construction.
- Do not claim a watcher branch is redundant until baseline-without-watcher and targeted mutation tests prove both positive and negative behavior.
- Preserve invalid-trailer error latching across async body, poll body, and trailer reads.
- Wire END_STREAM alone is never sufficient when h2 dropped payload, rejected trailers, or Content-Length remains unsatisfied.
- GOAWAY evidence is scoped by `last_stream_id`; no record may cross streams or connections.
- Local failures must not be overwritten into success by a remote `RST_STREAM(NO_ERROR)`.
- Failed or ambiguous upstream bodies must never complete cache admission.
- Keep upstream/main code, helper, dependency, and API choices authoritative unless an Edgion-only contract demonstrably requires a narrow adapter.
- Everything written to disk is English.
- Obtain independent design review before implementation and a fresh independent code review before closure.

## Required analysis outputs

1. A responsibility-to-test map for every current watcher state and consumer.
2. Minimum/current h2 public behavior report.
3. Watched versus baseline-without-watcher results for the complete contract matrix.
4. Mutation results showing which guards are load-bearing.
5. Three design alternatives: no watcher, narrow adapter, and retained watcher with an enforced upgrade gate.
6. Quantified code and hot-path complexity reduction for the selected design.
7. Dependency policy decision coordinated with upstream-main-authoritative maintenance.
8. Independent design-review result before any production change.

## Existing workspace caution

At task creation time:

- The Pingora `edgion_v3` worktree already contained a prerequisite modification
  to `pingora-core/src/protocols/http/v2/client.rs`: issue #2's poll API now
  latches invalid-trailer errors like the async API. It belongs to the H2
  completeness contract, predates this task, and must be preserved and
  revalidated before establishing the task baseline. The untracked
  `.superpowers/` directory is unrelated and must remain untouched.
- The Edgion worktree already contained unrelated tracked and untracked changes, including Cargo files, review knowledge, tasks, and wasm follow-ups.
- The review worktree's review directory was untracked as a whole.

Future agents must preserve these changes, must not stage them as part of this task, and must verify ownership before editing an overlapping file.
