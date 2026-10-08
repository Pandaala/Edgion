# Native WAF Request Verdict and Forwarding

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-23 |
| Status | completed; historical record superseded by the canonical regression guard |
| Type | feature |
| Priority | P0 |
| Issue | N/A |
| Design status | implemented and integrated into the supported fork; old publication notes below are historical |

## AI guide summary

Implement the first, narrow WAF request-body mode: inspect a bounded prefix
before selecting/connecting to an upstream and block on a match within that
prefix. The user refined the design during implementation: reuse the existing
registered request-body buffer and Edgion `BodyStorage`, with WAF-only storage
capacity equal to its inspection window. Add early prefix sealing and live
continuation without requiring complete capture. Preserve the complete-inspection
path for other configurations; later work can expand or reorganize modes.

## Pre-implementation baseline and transport facts

- `WafPlugin::run_request` currently streams eligible raw rules through the
  per-chunk request-body hook and otherwise calls `read_request_body` for a
  complete body. The streaming path can deny late, after upstream headers and
  earlier bytes have been sent; it does not use `BodyStorage`.
- The pinned Pingora fork has a 64 KiB H1 body-reader I/O buffer, an optional
  64 KiB H1/H2 retry buffer, and a 64 KiB maximum replay chunk. These are
  distinct. The H1 I/O buffer bounds a read, not total WAF inspection. The
  retry buffer tees bytes while forwarding and discards its content when
  truncated; it is not a pre-connect verdict mechanism.
- Pingora's registered request-body replay buffer, implemented by Edgion's
  `BodyStorage`, must be installed before body consumption and completely
  captured before upstream replay. It does not currently support consuming
  only a prefix, replaying that prefix, and then reading the remaining body
  directly from the downstream transport.
- The current 16 KiB values found in relevant fork code concern compression
  buffer reservations and HTTP/2 test frames, not a WAF body cap.
- The WAF engine's default body-byte limit is 16 MiB; the gateway's default
  physical body cap is 32 MiB. Do not conflate these limits with a Pingora
  transport buffer or a proposed pre-connect inspection window.

## Current design specification

The current [WAF request-body contract](../../skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md)
is authoritative for configuration, default early prefix caching, in-memory
window limits, full inspection, and exact forwarding. It supersedes the earlier
incremental spec in this task. The default captures only the inspection prefix
before dispatch, then forwards prefix, transport overrun, and live remainder
exactly once. Explicit `full` retains complete capture before the body verdict.

## First-slice implementation plan

1. **Pingora design and tests:** Specify and independently review the native
   prefix state machine: unarmed, collecting, admitted, first-attempt drain,
   live continuation, denied/abandoned. Preserve the configured prefix exactly,
   including a transport chunk that crosses the boundary; keep the remainder
   for normal forwarding. Prove cancellation safety, H1/H2 framing, real EOS,
   `Expect: 100-continue`, no duplicate first-attempt send, and retry policy.
2. **Shared validation:** Add the one policy enum to `WafConfig` and the WAF
   CRD subtree. Add a conservative leaf-crate prefix-eligibility check and an
   explicit prefix-completion API; do not publish a truncated prefix as an
   actual complete request body. Keep compilation off the request path.
3. **Gateway wiring:** Run the selected WAF's request-header phase, then the
   Pingora bounded prefix read and WAF prefix evaluation within the request
   stage. Only after admission permit upstream selection/connection. Freeze
   the request's body source, reuse window-bounded `BodyStorage` for this path, and preserve
   existing full-inspection behavior for explicit `inspection: full`.
4. **Regression and docs:** Use a recording backend to prove a prefix match
   sends zero upstream headers/body and an allowed request sends the exact
   prefix plus remainder once. Cover N-1/N/N+1, cross-chunk matches, short
   body EOS, unsupported rules, client cancellation, cap failures,
   `Expect: 100-continue`, H1/H2, retries, body mutation/other consumers,
   detection-only, and hot reload. Update English/Chinese WAF docs and the
   owning gateway/Pingora body contracts; measure latency and retained bytes.

The fork change must be integrated through the supported `edgion_v5` source
policy and locked to one revision. Do not leave a path override or an
unpublished fork commit as the final implementation. Run targeted tests,
format, workspace check/clippy, documentation validation, and policy guards
before closing this task.

## Configurable-window amendment

The user requested arbitrary `prefixSize` up to 1 MiB and retained explicit full
inspection. Prefix-only `BodyStorage` uses the window as its memory and total
capacity, including 1 MiB windows. Existing complete caches are reused; full mode
keeps the shared memory/disk capture policy and rejects `prefixSize`. Resolve the
size with the shared parser at configuration load and pass byte counts through
the Gateway and engine. Pingora's existing limit parameter requires no change.
Independent review approved the amendment and the parameterized 1 MiB hot-reload
matrix. Fresh local checks passed: workspace check/Clippy, formatting, engine
prefix-related tests (7), resource WAF tests (10), Gateway WAF-related tests (39),
CRD coverage (19), plugin CRD types (6), documentation, SSA, metrics inventory,
and Gateway API transform guards. The Gateway wire matrix remains unrun pending
test-topology approval; fork publication and final dependency integration remain
open. Local checks used the external temporary Cargo patch, then restored the
original published lockfile.

## Local verification status (before configurable-window amendment)

Implementation is present in Edgion and the sibling Pingora branch
`feat/waf-request-body-prefix`; neither repository has been committed or pushed
for this task. Independent review passed. Local validation with an external
temporary Cargo patch passed workspace check/Clippy, formatting, Gateway WAF
38 tests, resource WAF 9 tests, CRD coverage 19 tests, and documentation, SSA,
metrics, and Gateway API transform guards. Required binaries built successfully.
Pingora's request-body seam wire matrix passed 65 tests, plus focused prefix,
cancellation, and retry-refusal coverage.

The Gateway WafDynamic matrix is compiled but awaits approval to switch the
existing shared test topology. Fork publication through the supported source
policy and final dependency integration are still pending. Cargo.lock has been
restored to the original published revision; no repository-local path override
is retained. Its source-policy check validates that baseline only. Do not close
this task or treat an ordinary unpatched build as verified until publication
and final integration checks complete.

## Dependencies and context

Current dependency state: local `edgion_v5` consolidates the prefix feature and
discard fix into commit `c74d7fb2d59349b8cb1f7948102c7f9bbf829638`, with the same
tree as the reviewed feature branch. Edgion now selects that branch and commit
in its manifest and lockfile. No repository path patch is used. The older
verification sections above describe the historical temporary-patch baseline.
Remote publication and a clean remote-source build remain pending.
Fresh validation against the canonical Git source passed workspace check and
Clippy with `--locked`, 40 Gateway WAF-related tests, formatting, documentation,
SSA, metrics inventory, Pingora source policy, and Gateway API transform guards.
The unpublished Git commit was fetched locally for validation; no persistent
Git URL rewrite or repository path patch was installed.

## Gateway integration verification (2026-09-23)

The canonical Git dependency on local `edgion_v5` passed the static WAF suite
(5/5) and every `WafDynamic` phase on one shared Controller/Gateway topology:
initial prefix, full, invalid-generation retention, 1 MiB prefix, detection-only,
recovery, deletion, and recovery again. The wire matrix checks downstream
H1/H2 crossed with upstream H1/H2, exact length/digest and single delivery,
N-1/N/N+1 and boundary markers, zero upstream exposure on denial, and chunked
upload with local 100 Continue.

Actual execution found two fixture errors: the H1 backend reports `1.1`, and
H2 requests must use a domain URL plus a local address mapping to avoid a
conflicting IP authority and Host. Both corrections passed independent review;
address resolution also supports hostnames and IPv6. No Gateway or Pingora
implementation change was needed. Final-source formatting, workspace check and
Clippy passed with the canonical lockfile.

Run evidence: `tmp/integration_testing/testing_20260923_215343/` contains the
static report and supplemental WAF verification logs. Metrics role validation,
zero unknown config fields, requeue-loop scanning, and the final cache-residue
check passed. The residue check runs once after dynamic tests because it deletes
all resources. The original CLI token was restored and owned services stopped.
The task remains open for remote publication and clean remote-source validation.

Separate observation, not diagnosed in this WAF gate: an earlier attempt to
reload the entire topology after the runner's wipe-all produced route 404s even
though resource synchronization passed. Reproduction evidence is retained in
`tmp/integration_testing/testing_20260923_215138/`. Investigate whole-Gateway
delete/recreate independently; the WAF plugin delete/recovery scenario passed.

- Coordinate recording-backend cases with [production rule acceptance](waf-production-rule-acceptance.md).
- Parent: [native WAF roadmap](waf-self-built-implementation.md).
- Read `AGENTS.md`, `skills/SKILL.md`, `skills/00-design-rules.md`,
  `skills/04-review/http-body/SKILL.md`, and the pinned Pingora fork's
  request-body buffering and relay contracts before implementation.
