# Pingora HTTP/2 END_STREAM Watch Simplification

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-26 |
| Status | in-progress |
| Type | refactor |
| Priority | P2 |
| Issue | `p2-b9ba47c-end-stream-watcher-has-an-unenforced-h2-version-contract` |

## AI guide summary

Re-evaluate the Edgion Pingora fork's wire-level HTTP/2 END_STREAM watcher against the behavior now provided by upstream `h2 >= 0.4.16`. The current file contains approximately 779 lines of production documentation, wrapper, scanner, and state plus approximately 1,190 lines of tests and benchmarks; only production hot-path/state reduction counts as implementation simplification. Prove each response-integrity contract with baseline-without-watcher and mutation tests, classify every watcher responsibility, and then delete or sharply reduce only the production logic whose safety role is demonstrably redundant. The main difficulty is simplifying a hot-path security mechanism without reopening truncated-response, invalid-trailer, reset-ordering, GOAWAY, connection-reuse, or cache-admission failures.

## Review source and current baseline

- Historical review IDs: `p2-b9ba47c-end-stream-watcher-has-an-unenforced-h2-version-contract` and `p1-b9ba47c-invalid-h2-trailers-can-be-laundered-as-clean-eof`.
- Stable review ledger: `/Volumes/ExtStore/ws3/pingora-release-0.8-review/docs/edgion-fork/review-release-0.8/REMEDIATION-EDGION-V2.md` entries #2, #4, and #29.
- Invalid-trailer production anchor: Pingora `edgion_v3` commit `682506d`, plus the pre-existing poll-error-latch follow-up described below.
- Pingora source workspace: `/Volumes/ExtStore/ws3/pingora`
- Pingora branch at task creation: `edgion_v3` at `26024af2dac54bc3578b3d19c8c86af9d99269c0`
- Upstream baseline at task creation: `main` and `upstream/main` at `09696b51bc59315353d96686355861604d0bb48c`
- Upstream dependency declaration: `h2 = ">=0.4.16"`
- Locally resolved dependency at task creation: `h2 0.4.19` in an ignored, unversioned Pingora `Cargo.lock`
- Relevant upstream Pingora change: `b3317e3` raised the minimum h2 version from 0.4.14 to 0.4.16.
- Relevant h2 change: 0.4.16 preserves received END_STREAM evidence across a later reset while retaining reset information for the send half.

The old watcher's documentation was written against h2 0.4.15 and incorrectly claimed that a workspace lockfile pinned that version. The minimum supported h2 release and current resolved release now provide more complete reset behavior, but this does not prove that every Edgion wire-level safeguard is redundant. In particular, invalid trailers followed in the same burst by `RST_STREAM(NO_ERROR)` were reproduced under h2 0.4.19, and the current watcher also participates in byte-count, frame-order, GOAWAY, local-reset, and codec-error defenses.

## Requirements

1. Treat current upstream Pingora and its supported h2 API as the implementation baseline. Do not restore release-0.8.1 code or an older h2 behavior model.
2. Establish the exact public h2 behavior available at the supported minimum and current resolution before designing a replacement.
3. Build a baseline-without-watcher test mode that removes or bypasses wire evidence without deleting production code. Use it to identify contracts now supplied by h2 itself.
4. Add targeted mutation tests that disable one watcher proof or guard at a time. A passing happy-path suite is not evidence that the guard is redundant.
5. Classify each watcher responsibility as `upstream-covered`, `adapted`, `ported`, `obsolete`, or `needs-review`, with a test and source citation for every classification.
6. Prefer deleting or replacing the approximately 779 lines of production watcher documentation, wrapper, scanner, and state with h2 public behavior and small, explicit Edgion checks. If a narrow wire observation remains necessary, keep only the minimum parser/state required for the proven uncovered contracts. Track the approximately 1,190 lines of tests and benchmarks separately; deleting safety tests is not an optimization result.
7. Do not remove a security or correctness defense until both its positive and negative cases pass without it, including adversarial same-burst ordering.
8. Preserve response completeness, error latching, stream isolation, connection reuse, resource release, and cache-admission behavior.
9. Correct all stale comments and fork documentation so they describe the selected h2 support policy and the remaining evidence model accurately.
10. Decide dependency minimum, exact pin, upper bound, or CI matrix policy only during design review. The decision must respect the rule that upstream/main owns Cargo manifests by default; task creation does not authorize a Cargo change.

## Analysis method

### Phase 1: Contract inventory

- Map every field, transition, scanner branch, and caller of `EndStreamWatch`, `EndStreamWatchStream`, `StreamRecord`, and the client completion helpers.
- Map each rule to a positive test, a negative test, and the relevant h2 public behavior.
- Inspect the supported minimum h2 source and changelog as evidence, but do not make the final design depend on undocumented internals when a public observation can replace it.

### Phase 2: Baseline without the watcher

- Add a test-only construction path that uses the production h2 client/session without `EndStreamWatchStream`.
- Run the full contract matrix against both watched and unwatched paths.
- Record which cases are already correct under h2 0.4.16 and the current resolved version.
- Do not infer coverage from compilation or from one version resolving successfully.

### Phase 3: Mutation proof

- Disable or replace individual evidence sources independently: wire END_STREAM, delivered-byte count, terminal-trailer marker, reset/GOAWAY invalidation, local-reset invalidation, and error latching.
- Require each retained guard to have a mutation that makes its regression test fail.
- Treat a mutation that cannot be killed as evidence for further investigation, not automatic permission to delete code.

### Phase 4: Design and independent review

- Compare a no-watcher design, a narrow-adapter design, and retaining the existing scanner with an enforced upgrade gate.
- Quantify production hot-path/parser/state lines removed separately from test
  and benchmark lines, plus remaining private assumptions and per-read overhead.
- Obtain independent design review before implementation and independent code review before closing the original finding.

## Scope

### In scope

- The Edgion-only upstream HTTP/2 response completion behavior in the Pingora fork.
- `EndStreamWatch`, its IO wrapper, stream records, scanner/cache state, and all connector/client integration points.
- Current response body and trailer error-latching logic that consumes wire evidence.
- Contract, raw-wire, mutation, integration, and cache-admission tests needed to prove safe removal or reduction.
- Fork documentation and dependency-upgrade verification policy.
- A design-reviewed CI dependency matrix if it is selected without changing upstream-owned manifest semantics.

### Out of scope

- Changing Cargo manifests, pinning h2, adding an upper bound, or changing the minimum version during task creation.
- Downgrading Pingora, h2, Tokio, Hyper, or any other upstream dependency.
- Replacing main's current HTTP/2 connector, cancellation, flow-control, Content-Length, or GOAWAY architecture with old fork code.
- Removing the watcher solely because h2 0.4.16 fixed one reset-after-END_STREAM state transition.
- Broad HTTP/2 refactoring unrelated to the evidence and completion contract.
- Weakening malformed-response rejection, accepting ambiguous EOF, or permitting failed responses into cache.
- Changing Edgion product-facing configuration or plugin APIs.

## Required test matrix

Every protocol, cache, and lifecycle scenario must run with the production watch before simplification, the baseline-without-watcher path, and the final implementation. Mutation scenarios apply to current guards and every guard retained in the final implementation. The dependency matrix runs the design-approved contract suite at each supported dependency boundary.

| Area | Required scenarios | Required result |
|------|--------------------|-----------------|
| Complete response reset | DATA with END_STREAM followed by remote `RST_STREAM(NO_ERROR)` while request upload remains open | Clean EOF; delivered bytes preserved; request writer is awakened and stops without leaking a pump task |
| Truncated response reset | DATA without END_STREAM followed by remote `RST_STREAM(NO_ERROR)` | Body/trailer read remains an error; no clean EOF |
| Pre-poll reset | Complete and truncated variants delivered before the application first polls the body | Same classification as wire order requires |
| Trailer completion | Valid terminal trailers followed by remote `NO_ERROR` reset | Valid trailers/EOF preserved exactly once |
| Invalid trailers | Pseudo-only and pseudo-plus-regular trailer HEADERS with END_STREAM, with and without same-burst `RST_STREAM(NO_ERROR)` | Decoder failure remains latched for async, poll, and direct-trailer APIs; never clean EOF or successful trailer map |
| Flow-control drop | DATA carrying END_STREAM is decoded but dropped for stream flow-control violation, then reset | Dropped bytes cannot be vouched for as complete |
| Content-Length | Exact length, underflow, overflow, rejected trailers, and no declared length | Only exact, validated completion is accepted |
| GOAWAY stream ids | Stream below/equal to and above `last_stream_id`, split payload, malformed GOAWAY, graceful two-stage GOAWAY | Evidence retained or invalidated according to stream eligibility; no cross-stream leak |
| Local reset | Local protocol/flow-control reset followed by remote `NO_ERROR` | Remote reset cannot overwrite a local failure into success |
| Stream isolation | Interleaved streams with independent DATA, terminal frames, resets, and GOAWAY | One stream cannot publish, clear, or reuse another stream's evidence |
| Connection reuse | A complete response/reset exchange followed by a second stream on the same H2 connection; repeat after truncated/invalid failure | Successful exchange keeps the healthy connection reusable; failed stream is isolated and connection disposition follows the proven h2/main contract |
| Cache failure | Invalid trailers, truncation, flow-control drop, and local failure | No cache admission, cache marker completion, or reusable successful cache object |
| Lifecycle | Session drop, connection close, cancellation, EOF, reset, and GOAWAY | Records and buffers released; no stale evidence reused by another stream/connection |
| Mutation | Each retained evidence source or guard disabled independently | At least one targeted regression fails for every retained guard |
| Dependency policy | Supported minimum and default/current h2 resolution | Identical public Edgion behavior across the declared support set |

## Acceptance criteria

- [ ] The task records a complete responsibility map for the current watcher and all production consumers.
- [ ] A test-only baseline without the watcher establishes which behaviors h2 now provides directly.
- [ ] Each responsibility removed from production is classified `upstream-covered` with reproducible tests at every supported dependency boundary selected by design.
- [ ] Each retained responsibility has a mutation test that fails when its guard is removed or weakened.
- [ ] Complete response, truncated response, and pre-poll reset cases are deterministic and preserve their intended EOF/error classification.
- [ ] A response-complete reset wakes and terminates the still-open request-body writer without a leaked task, retained buffer, or stuck flow-control reservation.
- [ ] A healthy H2 connection accepts a second stream after the complete-response reset case; truncated and invalid cases prove the intended stream isolation and connection disposition explicitly.
- [ ] Invalid trailers in the same burst as `RST_STREAM(NO_ERROR)` remain a permanent error through async body, poll body, and trailer APIs.
- [ ] Flow-control-dropped DATA and Content-Length mismatches cannot be accepted using wire END_STREAM alone.
- [ ] GOAWAY handling is proven for stream ids below/equal to and above `last_stream_id`, including split and malformed frames.
- [ ] Local reset and connection teardown cannot be overwritten into clean completion by a later remote `NO_ERROR` reset.
- [ ] Failed or ambiguous responses cannot complete cache admission or leave a reusable successful cache entry.
- [ ] Stream and connection lifecycle tests prove no stale evidence or resource retention after completion, reset, cancellation, or drop.
- [ ] The final implementation removes the full wire parser or documents, with killed mutations, why each remaining narrow observation is necessary.
- [ ] Production watcher documentation/wrapper/scanner/state reduction is reported separately from test/benchmark changes, using the approximately 779/1,190 creation-time split as a reproducible baseline; line count is an outcome metric, not permission to remove an unproven defense.
- [ ] Cargo manifest policy is explicitly approved in design review and remains consistent with upstream-main-authoritative maintenance.
- [ ] Fork documentation no longer claims a tracked Cargo.lock or the obsolete h2 0.4.15 `recv_reset` behavior.
- [ ] Targeted core, proxy integration, cache, format, check, clippy, and documentation consistency checks pass.
- [ ] A fresh independent reviewer confirms that no response-integrity or cache-admission contract was lost.

## Affected modules

| Module | Impact |
|--------|--------|
| `/Volumes/ExtStore/ws3/pingora/pingora-core/src/protocols/http/v2/end_stream_watch.rs` | Primary parser/state removal or reduction; scanner and mutation-test ownership |
| `/Volumes/ExtStore/ws3/pingora/pingora-core/src/protocols/http/v2/client.rs` | Completion proof, body/trailer error latching, public h2 behavior integration |
| `/Volumes/ExtStore/ws3/pingora/pingora-core/src/connectors/http/v2.rs` | IO wrapping, watch registration, connection lifecycle |
| `/Volumes/ExtStore/ws3/pingora/pingora-core/src/protocols/http/v2/server.rs` | Shared reset/completion assumptions and regression compatibility |
| `/Volumes/ExtStore/ws3/pingora/pingora-proxy` tests and cache paths | Cross-layer response failure, connection reuse, and cache-admission verification |
| `/Volumes/ExtStore/ws3/pingora/docs/edgion-fork` | Evidence model, h2 support policy, and upgrade gate documentation |
| Pingora CI or fork-specific verification scripts | Minimum/default dependency contract, only if approved in design review |

## Non-goals

- Making `edgion_v3` resemble the old `edgion` implementation.
- Preserving a large parser for historical reasons when public h2 behavior and tests make it redundant.
- Deleting a guard to meet a line-count target before its mutation is killed.
- Maintaining two independent response-completion implementations.
- Treating an ignored local Cargo.lock as a library-consumer version guarantee.
- Solving unrelated response filter, request buffering, PROXY protocol, or test-harness findings.

## Open design decisions

1. Can the watcher be removed completely under h2 0.4.16+, or do invalid-trailer, dropped-DATA, or GOAWAY cases require a narrow wire adapter?
2. Which h2 public observations replace each current private-state assumption?
3. Should the supported dependency policy remain upstream's unbounded `>=0.4.16`, use a fork CI minimum/latest matrix, or request an upstream-compatible manifest change?
4. What is the smallest deterministic GOAWAY and same-burst raw-wire suite that serves as the dependency upgrade gate?
5. Where should cache-admission failure tests live so they exercise the actual proxy cache lifecycle rather than only core body reads?
6. What measurable parser/state and hot-path overhead reduction is required to justify the refactor risk?

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| [00-context.md](00-context.md) | done | Source anchors, required skills, review findings, and safety constraints |
| [01-design.md](01-design.md) | in-progress | Responsibility map, minimum/current baseline, mutations, and candidate design |
| 02-implementation.md | not created | Create only after independent design review |
| [03-subtasks.md](03-subtasks.md) | in-progress | Analysis, core adapter, proxy/cache, dependency gate, and review ownership |
| [04-h2-decoder-remedy.md](04-h2-decoder-remedy.md) | in-progress | Minimal candidate decoder repair and temporary-overlay evidence |
| 04-todo.md | not created | No scattered follow-ups yet |
| [05-issues.md](05-issues.md) | in-progress | Blocking h2 trailer pseudo-header decoder defect |
| [06-decisions.md](06-decisions.md) | in-progress | First independent design review rejected production implementation pending P1 evidence |
| 07-changelog.md | not created | Create when implementation begins |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-26 | Promote review finding #4 to a feature-profile task | Safe simplification requires dependency research, baseline/mutation proof, protocol design, cache verification, and independent review rather than a local manifest edit |
| 2026-08-26 | Keep upstream/main authoritative for Cargo at task creation | Current Pingora main already owns `h2 >= 0.4.16`; exact pinning or an upper bound is a separate design decision with long-term fork cost |
| 2026-08-26 | Optimize for deletion or sharp reduction of the wire parser | h2 0.4.16 added stronger reset-after-END_STREAM behavior, so fork-only machinery must justify itself contract by contract |
| 2026-08-26 | Require proof before deleting any safety defense | h2 0.4.19 still reproduced invalid-trailer laundering, so one upstream fix does not prove the whole watcher obsolete |
| 2026-08-26 | Prefer a narrow wire adapter for design review | h2 0.4.16 and 0.4.19 preserve complete DATA END_STREAM without the watcher, while both still launder invalid terminal trailers after a same-burst reset; the proxy request writer still needs a pre-h2 wire completion signal |
| 2026-08-26 | Keep implementation paused after first design review | Reviewer found unresolved P1 contracts for terminal HEADERS, writer ordering, GOAWAY, upstream reuse, cache admission, and dependency enforcement; test-only analysis continues |
| 2026-08-26 | Block watcher simplification on an h2 decoder remedy | Both supported h2 versions and current upstream master discard pseudo-fields before exposing ordinary trailer fields; a temporary minimal decoder patch passes pseudo-only and mixed pseudo trailer contracts at both dependency boundaries |
