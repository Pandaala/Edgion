# Integration Verification — Handoff Brief

| Key | Value |
|-----|-------|
| Created | 2026-08-03 |
| Status | handoff — the code is written, the end-to-end verification is NOT done |
| Goal | Take the transport-coverage test build-out from "compiles and unit-tests pass" to "the integration suite runs green, repeatedly, with assertions that can actually fail" |

This is a working brief, not a summary. Everything an agent needs to finish the
job is here; you should not need to reconstruct the history.

## 1. The three layers, and which are proven

| Layer | Where | State | Proven? |
|---|---|---|---|
| Pingora fork: request-body transport seam | `/Users/caohao/ws2/pingora`, branch `edgion`, commit `b1d520e` | committed | **yes** — 43 seam + 32 proxy-lib + 381 core-lib tests pass |
| Pingora fork: test harness (event recorder + protocol matrix) | same repo, **uncommitted**: `M tests/test_request_body_seam.rs`, `?? tests/seam/` | uncommitted | **yes** — included in the 43 above |
| Edgion: ExtProc `STREAMED` port | `/Users/caohao/ws2/Edgion`, branch `feature-06-24` | committed | reviewed twice, unit-tested |
| Edgion: transport test backends + client primitives + scenarios | same repo, **uncommitted** (see §2) | uncommitted | **NO — this is the job** |

The pingora side needs nothing further. **The unverified surface is entirely
Edgion's integration suite.**

## 2. Uncommitted Edgion changes to verify

```
 M edgion-tests/integration/scripts/integration/run_direct.sh
 M edgion-tests/integration/scripts/utils/start_all.sh
 M edgion-tests/integration/scripts/utils/start_all_with_conf.sh
 M edgion-tests/src/client/framework.rs
 M edgion-tests/src/client/suites/edgion_plugins/canary/dynamic.rs
 M edgion-tests/src/client/suites/http_route/basic.rs
 M edgion-tests/src/server/test_server.rs
 M skills/05-testing/01-integration-testing.md
 M skills/05-testing/references/test-server-capabilities.md
?? edgion-tests/src/server/transport/
```
plus whatever the in-flight scenarios pass adds under
`edgion-tests/src/client/` and `edgion-tests/integration/conf/`.

**Do not commit anything.** The repository owner commits.

## 3. What was built (so you can tell a real failure from a misunderstanding)

**Two dedicated raw backend ports on `test_server`**, deliberately not on the
axum router — axum cannot express the misbehavior these tests need:
- **30030** — hand-written HTTP/1.1 over a tokio `TcpListener`
- **30031** — `h2::server`

Steered by request headers (the repo's existing `x-…` control idiom):

| Header | Values |
|---|---|
| `x-up-behavior` | `echo`, `early-response`, `hang`, `half-close`, `reset`, `close`; H1-only: `malformed-cl-and-te`, `malformed-chunk-size-{overstated,understated}`, `malformed-chunk-terminator-{missing,corrupt}`, `malformed-content-length-{overstated,understated}` |
| `x-up-at-bytes` | byte offset at which the behavior fires |
| `x-up-reset-code` | RST_STREAM error code |
| `x-up-after-response` | `linger` (default), `close`, `reset`, `goaway` |
| `x-up-delay-ms`, `x-up-status`, `x-up-resp-len`, `x-up-drain` | modifiers |
| `x-trace-id` | correlation key (already used elsewhere in this harness) |

**Transport event recorder**, queried on the existing axum router:
```
GET /transport/query/{trace_id}
  -> { found, count, events[], connections[{listener, conn, requests, traces, closed}],
       conn_accepted_total{h1,h2}, dropped }
GET /transport/reset      # clears events and traces; NEVER the global accept counter
```
Load-bearing detail: the query returns the trace's own events **plus the
untagged connection-scoped events of every connection that carried that
trace**. Without that, connection reuse cannot be asserted at all — and reuse
is one of the three gaps this whole effort exists to close. Use
`conn_accepted_total` as a **delta** across a scenario, never as an absolute.

**Framework**: `TestFn` was widened from a bare `fn` pointer to
`Arc<dyn Fn(TestContext) -> TestFuture + Send + Sync>`, and `TestCase::new` is
generic. This is what allows one scenario body to run across protocol
combinations instead of being hand-copied four times. 1002 existing call sites
were unaffected. A minimal example case lives in `suites/http_route/basic.rs`
— if it ever fails to compile, someone re-narrowed the type.

**Protocol combinations**: a combination is (client constructor) × (hostname).
Downstream H1/H2 needs no YAML (h2c is on by default for HTTP listeners);
upstream is one Service field (`appProtocol: kubernetes.io/h2c`).

## 4. Run it

The macOS `lo0` prerequisite **is already satisfied on this machine**
(127.0.0.2/3/4 aliases present) — if a run fails, it is not that.

```bash
cd /Users/caohao/ws2/Edgion

# 1. Static gates first — these abort run_integration.sh before any test runs
cargo fmt --all -- --check
cargo check -p edgion-tests --all-targets
cargo clippy -p edgion-tests --all-targets
python3 cicd/checks/validate_agent_docs.py
cicd/checks/check_english_only.sh
cicd/checks/check_bash_set_e_increment.sh

# 2. Unit tests — report the FULL output, not a grepped fragment.
#    (A prior verification pass sampled this with head/tail and reported a
#     number it had not actually seen. Do not repeat that.)
cargo test -p edgion-tests

# 3. The real thing: the integration suite for the new scenarios
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>

# 4. And the suites the new backends could plausibly disturb
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i ExtProc
```

Replace `<Resource>/<Item>` with whatever the scenarios pass registered; the
registration sites are listed in
`/private/tmp/claude-501/-Users-caohao-ws2-pingora/03642a2c-bedf-433e-8a67-ace6057016bd/scratchpad/edgion-transport-coverage-plan.md`.

**Repeat runs are mandatory.** Run each new scenario at least 10 times and
report the actual pass count. This effort has already shipped, twice, a test
that passed once and was either flaky (5 of 6 isolated runs failed) or
structurally vacuous. A single green run is not evidence.

## 5. Hard rules — do not relax these to get green

1. **Every negative claim needs a real assertion.** "No second connection was
   opened" must be `expect_none` over a bounded window **plus** an exact
   whole-log count. A bound like `count <= 1` is satisfied by zero and proves
   nothing. This exact defect has already been found twice in this codebase.
2. **Every scenario must first prove the upstream received the request**,
   before asserting anything about how the exchange ended. Otherwise a test
   that never reached the upstream passes silently.
3. **A skipped combination must announce itself with its reason** and must not
   count as coverage.
4. **If a test fails, diagnose before adjusting the assertion.** The three
   review rounds behind this work found real defects — a failing assertion is
   more likely to be right than convenient. If you weaken an assertion, say so
   explicitly and say why.
5. **Do not touch production code** (`edgion-gateway`, `edgion-resources`,
   `edgion-common`, or the pingora fork's `src/`). If something cannot be
   observed without a production change, that is a finding to report, not a
   licence.

## 6. Do NOT implement these (already decided, with reasons)

CONNECT framing variants (Gateway API has no CONNECT route type — the path is
unreachable from any Edgion config); a downstream body idle-timeout test (a
deliberate open gap in the fork — a test would freeze a deficiency as expected
behavior); custom-connector coverage (unreachable from any Edgion route);
K8s-mode parity for ports 30030/30031; a gateway-side reuse metric (the repo's
own skill forbids adding metrics to verify behavior, and the recorder observes
the wire rather than the gateway's belief about it).

Full reasoning: `tasks/todo/transport-coverage-observability-gaps.md`.

## 7. What the assertions are ultimately proving

`09-pingora-interface-contract.md` §3 (what the proxy does automatically), §4
(limits and gotchas), §6 (deltas for code that does not use the interface).
Read those three sections before judging whether an assertion is testing the
right thing.

## 8. Definition of done

- Every static gate in §4 step 1 passes.
- `cargo test -p edgion-tests` passes, with the full output reported.
- The new integration suite runs green **10 times**, with pass counts reported.
- The pre-existing ExtProc suite still runs green (no disturbance from the new
  backends or the `TestFn` widening).
- Every skip is listed with its reason; every assertion weakened during
  debugging is listed with its justification.
- Anything that could not be verified is stated plainly rather than omitted.
