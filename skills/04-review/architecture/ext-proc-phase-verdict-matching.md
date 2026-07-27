---
name: ext-proc-phase-verdict-matching
description: Use when reviewing ExtProc streaming exchanges; covers strict request/response phase matching before mutations are applied.
status: fixed
finding_id: BODY-003
closed: 2026-07-27
---

# BODY-003 — ExtProc Phase Verdict Matching — FIXED

## Conclusion

Every ExtProc `ProcessingResponse` must match the outstanding
`ProcessingRequest` phase. Wrong, duplicate, reordered, empty, or missing
verdicts fail through `failureModeAllow` before any response from the exchange
is applied; `ImmediateResponse` remains terminal at every phase.

Fix suggestions that accept the right number of responses without matching
their variants are **not accepted**.

## Core Rationale

**1. Message count is not a policy verdict**

A headers-plus-body exchange requires one headers verdict and one body verdict.
Two headers responses have the expected count but provide no body policy
decision. `ExtProcClient::exchange_all` therefore derives an ordered
`ProcessingPhase` sequence from the outbound messages and validates each
inbound variant against it.

**2. Validation precedes mutation**

The client collects only a completely valid exchange. A later wrong-phase or
missing response returns `FailedPrecondition` and discards earlier responses,
so the plugin cannot partially apply a headers mutation before discovering that
the required body verdict is absent.

**3. Protocol failures use the configured dependency policy**

Phase mismatch and premature stream close are classified as `bad verdict`;
transport and timeout errors remain `unreachable`. Both classifications pass
through ExtProc's existing `failureModeAllow` choke point.

## Fix Suggestions Not Accepted

- Stop after receiving the expected message count — does not prove that every
  required phase was decided.
- Validate variants only inside the plugin application loop — can expose
  partial mutations before a later invalid verdict is detected.
- Treat `ImmediateResponse` as a wrong-phase verdict — it is the protocol's
  valid terminal response for any active phase.

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
