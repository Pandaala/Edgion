# Integration Verification — Results (2026-08-03)

Verification pass against `12-integration-verification-handoff.md`. Everything
below was run on branch `feature-06-24` with the pingora fork working tree at
commit `b1d520e` **plus its uncommitted H2-client changes**
(`end_stream_watch.rs`, `client.rs`, `connectors/http/v2.rs` — the in-flight fix
for gap 9), which the gateway picks up through the `[patch]` path dependency.
Gateway/controller binaries built 22:11; scenario code last modified 18:29.

## Static gates (§4 step 1) — all pass

- `cargo fmt --all -- --check` — clean. (Two formatting diffs found in
  `edgion-tests/src/client/transport.rs` at the start of the pass were fixed
  with `cargo fmt -p edgion-tests`; no other file needed changes.)
- `cargo check -p edgion-tests --all-targets` — pass.
- `cargo clippy -p edgion-tests --all-targets` — the new transport code is
  warning-free after fixing two `useless use of vec!` warnings
  (`transport.rs:1811,1894`, `&vec![..]` → `&[..]`). The remaining ~81 warnings
  in the `edgion-tests` lib are pre-existing across old suites (they surface
  now only because `cargo clean` invalidated the warm clippy cache that had
  been hiding them); `test_server.rs:2232` `manual_let_else` predates this work.
- `validate_agent_docs.py`, `check_english_only.sh`,
  `check_bash_set_e_increment.sh` — pass.

## Unit tests (§4 step 2)

`cargo test -p edgion-tests`: **122 passed, 0 failed** across 8 test binaries
(lib 41, build_test_proto 8, cache_residue 22, config_load_validator 7,
resource_diff 44, test_client/test_client_direct/test_server 0 each). The 46
transport unit tests (`transport::{h1_wire,h2,recorder,script}::*` server-side,
`transport::tests::*` probe-side) are all in the passing set.

## Integration runs

All runs used the same 22:11 gateway binary. Runs that collided with a second
agent concurrently driving the shared stack (mutual `pkill -9` in the launch
scripts) are excluded as environmental; 8 runs completed cleanly.

**HTTPRoute/Transport — 72 cases: the 70 non-gap-9 cases passed in 8/8 runs.**
Per-run failure counts: 2, 1, 1, 2, 0, 2, 2, 2 — every failure was one or both
of `upstream_reset_no_error_after_complete_response__{h1_to_h2,h2_to_h2}`
(gap 9). One run was 72/72.

**Gap 9 status under the in-flight fork fix: still failing, now flaky.** The
gateway error is `Fail to proxy: Upstream H2Error … while reading h2 header
cause: stream error received: not a result of an error, status: 502` — the
`RST_STREAM(NO_ERROR)` is processed by the `h2` connection task **before the
user task consumes the buffered response HEADERS**, so `poll_response` itself
errors and the complete buffered response is discarded inside the `h2` crate.
An EOF latch in the fork cannot recover a response the `h2` crate never
delivers; a complete fix appears to need `h2`-level behavior (deliver a fully
buffered response before surfacing a post-EOS reset). The assertion is correct;
do not weaken it.

**EdgionPlugins/ExtProc — 13/13 pass.** The pre-existing ExtProc suite
(including STREAMED) is undisturbed by the transport backends, the `TestFn`
widening, and the in-flight fork changes.

**HTTPRoute/Basic — 6/6 pass**, including `captured_parameter_closure` (the
guard that fails to compile if `TestFn` is re-narrowed to a `fn` pointer).

## Assertion-quality review (§5 hard rules) — no violations found

Read in full: the suite (`suites/http_route/transport/transport.rs`), the
client primitives (`client/transport.rs`), the server backends
(`server/transport/*`). Findings:

- Every negative claim is positive-twin + exact whole-log count + cursor-window
  `expect_none`; no `<= n` bounds anywhere.
- `expect_none` takes its cursor at call time (empty scoped log degrades to a
  whole-log window — stronger, never weaker); `wait_for` fast-fails on
  foreclosure; both are `#[must_use]`. The sibling-trace query-scoping trap is
  handled by `expect_none_for` with the reason documented.
- Exercised-upstream guard + declared/absent-kind runtime checks wrap every
  case; naming↔kinds and skip/coverage-overlap rules are unit-tested;
  `EXPECTED_TRANSPORT_CASES` (72) is double-checked (const + derived).
- All 12 skips announce truthful reasons. Note: skipped cases count into the
  report's "Passed" total (clearly labeled `SKIPPED` per case) — acceptable,
  but a reader of the raw totals should know 72 = 60 executed + 12 skips.
- One 300 ms sleep (`transport.rs` poison shape) only avoids a false failure;
  it cannot create a false pass. All ordering claims anchor on recorder `seq`.
- `client_pipelines_second_request` and `client_expect_100_continue` pin
  discovered behavior (pingora refuses pipelining via `has_bytes_overread`;
  no interim on unbuffered routes) with hand-verified evidence and source
  references — correct handling, not assertion-weakening.

## Deviations / caveats

- Files changed during verification (all test-layer, none production):
  formatting in `client/transport.rs`, two `vec!`→array literals in the suite.
- The 10-consecutive-green-runs criterion is not literally achievable while
  gap 9 is open: the suite exits 1 by design on the two known-failing cases.
  The operative criterion used instead: zero non-gap-9 failures across all
  clean runs (met, 8/8), plus ExtProc and Basic green.
- Repeat-run counts were capped by contention with a second agent concurrently
  running the same suite on the shared single-stack scripts; collided runs
  show `Killed: 9` startup failures and were excluded.
