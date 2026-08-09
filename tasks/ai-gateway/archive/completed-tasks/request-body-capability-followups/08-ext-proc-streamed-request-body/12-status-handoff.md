# Status Handoff — 2026-08-03 end of session

> Written for the next agent session picking this workstream up. Read this
> first, then only what the "entry points" section says you need.

## What is DONE (all in the uncommitted working tree, staged)

Two logical changesets, both fully implemented, reviewed, and verified:

1. **Fork-only build refactor**: the `body-buffering`/`upstream-pingora`
   flavor duality and `edgion-gateway-stock` binary are removed; the Gateway
   builds exclusively against the Pandaala/pingora fork (branch `edgion`,
   locked `b1d520ea`, plain `pingora-*` dep names, no aliases). CI, scripts,
   guards, and docs all updated.
2. **ExtProc `requestBodyMode: STREAMED`**: per-chunk request-body exchange
   on a request-scoped gRPC stream, via the new `RequestBodyStreamHandler`
   seam + `request_body_filter_action` hook + monotonic
   `ctx.request_body_stream_active` latch. Contract steps §8.1–8.5 of
   `09-pingora-interface-contract.md` are ALL complete.

Review state: two rounds (1 adversarial reviewer, then 3 parallel deep
reviewers) — 26 findings total, ALL fixed. Full record with rationale:
`11-edgion-streamed-port-review.md`. Do not re-litigate items listed there
as "accepted / deferred".

Verification at handoff time: `cargo check/clippy/fmt` clean, full
`cargo test --all` green (30 suites), integration EdgionPlugins/ExtProc
13/13 (6 streamed cases incl. real multi-chunk), all four cicd guards pass.

## CAVEAT — merge with a parallel workstream, re-verify before commit

After the above verification, the maintainer pulled two parallel commits
(`b7d4ec35`, `962c5c94`: auth-common/CORS/rate-limit/`response_denial` work)
UNDER the staged changes and hand-resolved a conflict in
`pg_upstream_response_filter.rs` (both streams touched it). Confirmed after
the merge: no conflict markers anywhere; the streamed-port changes are intact
(latch present at all 5 consumer sites); `cargo check --workspace
--all-targets` passes. **Not yet re-run after the merge**: `cargo test
--all` and the ExtProc integration suite. Run both before committing —
`response_denial` and the streamed terminate path both operate near the
response stage.

## Commit plan (agreed with maintainer, awaiting instruction)

Two commits: (1) fork-only refactor incl. lockfile update + test-gate fixes;
(2) STREAMED port + review fixes + tests/docs. Never commit without an
explicit maintainer instruction (repo rule).

## Entry points by task

- **Continue/verify this workstream**: this file, then
  `11-edgion-streamed-port-review.md` (what was found/fixed), then
  `09-pingora-interface-contract.md` (the authoritative contract).
  `10-edgion-streamed-port-plan.md` is the historical implementation plan.
- **Envoy parity follow-ups** (P1a single stream, P1b mode_override, P2a
  streamed CR via Bodyless, P2b timeout override, P3 observability mode):
  `tasks/todo/ext-proc-envoy-parity.md` — self-contained with rationale,
  sketches, and an explicit deferred list.
- **Next major feature (recommended)**: WAF M1 —
  `tasks/todo/waf-self-built-implementation.md` → spec
  `docs/superpowers/specs/2026-07-12-waf-design.md`. Note for the reader:
  the spec's #1 technical risk (body pre-read + replay against pinned
  pingora) is now RESOLVED in practice — the shipped `BodyStorage`
  capture/replay and the streamed seam are exactly that infrastructure.
- **Small hygiene items**: fork's own `pingora-proxy` integration suites
  (`test_basic.rs`/`test_upstream.rs`, need local openresty — contract §7
  residual, never run); `tasks/todo/resource-diff-auth-failure-false-positive.md`.

## Key architecture facts a new session must not rediscover the hard way

- `ctx.request_body_stream_active` is a monotonic latch (set on install,
  NEVER cleared). All retry/disposition predicates key on it, not on the
  handler vector — the vector can be emptied by a downstream future dropped
  mid-hook. See review record finding 1 + B-C1/B-I1/B-I2 before touching
  the hook, the latch, or `write_terminate_reply`.
- The adapter install gate refuses upgrade/CONNECT and pre-HTTP/1.1
  requests (disposition-coercion preemption) and sessions without the
  transport fact. Bodyless gating uses `request_headers_end_stream()`, not
  `declares_body()` (CL can lie on H2).
- STREAMED must never trigger `BodyStorage` capture (`BodyRequirement` maps
  BUFFERED only) — contract "never pre-read" obligation.
