---
name: ext-proc-phase-verdict-matching
description: Use when reviewing ExtProc streaming exchanges; covers strict per-phase request/response matching and mutation boundaries.
status: fixed
finding_id: BODY-003
closed: 2026-07-27
knowledge_type: regression-guards
---

# BODY-003 — ExtProc Phase Verdict Matching — FIXED

## Conclusion

Every ExtProc `ProcessingResponse` must match the outstanding
`ProcessingRequest` phase. Wrong, duplicate, reordered, empty, or missing
verdicts fail through `failureModeAllow` before that phase's response is
applied; `ImmediateResponse` remains terminal at every phase.

This is a per-phase guarantee, not a whole-exchange transaction. A mutation
from an earlier valid phase may already be visible when a later phase fails;
the current `AttachedExtProc` / `ExtProcStream` design does not promise
cross-phase rollback.

Fix suggestions that accept the right number of responses without matching
their variants are **not accepted**.

## Core Rationale

**1. Message count is not a policy verdict**

A headers-plus-body flow requires one headers verdict and one body verdict.
Two headers responses do not provide a body policy decision. For each
outstanding request, `ExtProcStream::exchange` derives its `ProcessingPhase`
and accepts only the matching response variant (or a terminal
`ImmediateResponse`). `AttachedExtProc` separately validates the ordering of
phases sharing the request-scoped stream.

**2. Validation precedes the current phase's mutation**

Each `exchange` returns only after its response variant matches the outstanding
request. The owning plugin path then validates and applies that phase's common
response before advancing. A later wrong-phase or missing response fails the
remaining flow, but it does not undo an already applied headers mutation from
an earlier valid verdict.

**3. Protocol failures use the configured dependency policy**

At the stream boundary, phase mismatches return `FailedPrecondition`; premature
stream closure, transport errors, and timeouts use their corresponding tonic
statuses. Every consumer routes these failures through the configured policy,
but their local reason labels are not uniform: request-side paths distinguish
`bad verdict` from `unreachable`, while the post-commit response-body and
trailer adapters currently summarize exchange errors as `dependency`.

## Fix Suggestions Not Accepted

- Stop after receiving the expected message count — does not prove that every
  required phase was decided.
- Defer phase matching to ad hoc plugin branches — weakens the shared stream
  boundary and risks inconsistent handling across request, response-body, and
  trailer consumers.
- Treat `ImmediateResponse` as a wrong-phase verdict — it is the protocol's
  valid terminal response for any active phase.

## Verification Boundaries

The client state-machine tests verify per-message phase matching, ordered use
of one stream, final half-close, and rejection of extra terminal verdicts.
Request/response body and trailer tests exercise their local wrong- or
missing-verdict failure paths. These tests do not establish atomic commit or
rollback across multiple phases, and this page must not be used as evidence of
such a guarantee.

## Re-evaluation Triggers

Re-open this decision only if:

- The ExtProc protocol permits out-of-order responses with explicit request
  correlation identifiers.
- Multiple phase exchanges become concurrent on one stream.
- A new response variant gains terminal semantics.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-003-ext-proc-phase-matching.md`
- Client state machine:
  `edgion-gateway/src/plugins/http/ext_proc/client.rs`
- Failure policy: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`
- Integration coverage: BodyPlugins duplicate-header, wrong-order, and
  missing-body verdict scenarios.

## Resolution references

- `BODY-003` — 2026-07-27.
- `correct-ext-proc-phase-exchange-knowledge` — updated the historical batch-
  exchange description to the current per-phase exchange and mutation boundary.
