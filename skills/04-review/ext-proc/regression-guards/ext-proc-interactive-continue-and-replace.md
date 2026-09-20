---
name: ext-proc-interactive-continue-and-replace
description: Use when reviewing ExtProc request-body replacement and CONTINUE_AND_REPLACE semantics; covers publication ordering, owner terminalization, interactive phase sending, stream half-close, and bounded response transport windows.
status: fixed
finding_id: BODY-004
closed: 2026-07-27
knowledge_type: regression-guards
---

# BODY-004 — ExtProc Interactive Continue-and-Replace — FIXED

## Conclusion

ExtProc phases on one direction are sent interactively on one bidirectional
gRPC stream. The gateway waits for the matching headers verdict before sending
the original body; `CONTINUE_AND_REPLACE` applies the valid request mutations
and stops that direction, while the final outbound phase immediately
half-closes the request stream.

If a valid request-body replacement cannot be published to the canonical
request `BodyStorage`, the replacement and later header mutation are not
applied. The configured failure policy still chooses fail-open or fail-close,
but the shared request/response owner is terminalized in both cases so the
access-log summary records the local request-stage failure and no response
companion continues the transaction.

WP1 adds bounded response transport-window mutation, but response-header and
response-body `CONTINUE_AND_REPLACE` remain explicitly unsupported: their
stop-sending semantics are not equivalent to an ordinary per-window mutation.

## Core Rationale

**1. A headers verdict controls whether the original body is disclosed**

Submitting headers and body together makes `CONTINUE_AND_REPLACE` impossible:
the processor has already received the body it instructed the proxy to discard.
The shared `AttachedExtProc` sends each request through
`ExtProcStream::exchange`, which waits for a matching verdict before the caller
may send the next phase. The caller applies a valid headers verdict before the
body exchange; this ordering intentionally does not imply whole-exchange
rollback if a later body phase fails.

**2. Terminal status and stream closure are separate requirements**

`CONTINUE_AND_REPLACE` and `ImmediateResponse` stop later phases. Independently,
when the last configured phase is sent, the mpsc sender is dropped immediately
so a processor that waits for inbound EOF before returning its final verdict
does not deadlock with the gateway.

**3. Mutations follow their protocol phase**

A request-headers body mutation takes effect only with
`CONTINUE_AND_REPLACE`. A buffered request-body response may apply both header
and body mutations. Body publication happens before header mutation so a
rejected body version cannot leave half of the same verdict applied.

**4. Unsupported terminal replacement status fails explicitly**

Edgion streams proxied upstream responses. WP1 applies ordinary
`ResponseBodyResponse.body_mutation` to one bounded transport window at a time
without response-side storage. Accepting `CONTINUE_AND_REPLACE` while ignoring
its direction-level stop semantics would still be a protocol violation, so it
is routed through the configured failure policy. This is not semantic SSE
windowing; that awaits the canonical raw-byte-preserving hold/release adapter.

**5. Local publication failure supersedes a successful wire verdict**

A successful `RequestBodyResponse` proves only that the processor produced a
verdict. It does not prove that the gateway committed the replacement. The
wire exchange may already have recorded `success/normal-eos`, or may have left
the owner open for response phases. `apply_request_common_response` therefore
routes a failed `overwrite_request_body` through `fail_request_owner` whenever
an attached owner exists. This overwrites any premature success summary and
deactivates response-header, response-body, and trailer companions without
changing the original `BodyStorage`, the generic 500 response, or fail-open
continuation semantics.

## Fix Suggestions Not Accepted

- Send all request phases before reading verdicts — exposes the original body
  before a headers replacement decision.
- Open a separate gRPC stream per headers/body phase — loses transaction state
  and is not the ExtProc bidirectional-stream contract.
- Leave the sender open until the verdict arrives — deadlocks processors that
  wait for the request stream's final EOF.
- Ignore response-header replacement bodies — silently claims support for a
  mutation Edgion cannot apply.
- Apply `failureModeAllow` without terminalizing the attached owner after a
  local request-body publication failure — preserves a misleading success or
  permits later response phases after ExtProc has already failed for the
  request.

## Re-evaluation Triggers

Re-open the terminal-status decision only if:

- Direction-level response replacement/stop-sending semantics are specified.
- ExtProc adds another terminal response status.
- The client moves from ordered phases to explicit correlation identifiers.
- `AttachedExtProc` no longer owns both request and response phases, or response
  companions stop using its terminal state as their deactivation boundary.
- Request-body replacement bypasses `BodyStorageView` or its publication API no
  longer rejects failed candidates before changing the current generation.

## Verification Boundaries

The ExtProc publication-failure tests deterministically mock
`overwrite_request_body` rejection. They verify policy results, terminal
summary replacement, skipped later header mutation, and response-companion
deactivation without opening a network connection. They do not independently
prove storage atomicity.

The unchanged-body and unchanged-generation guarantee belongs to
[BODY-002 — Request-Body Mutation Physical Cap](../../http-body/regression-guards/request-body-mutation-physical-cap.md).
Its `BodyStorageView` implementation evidence and
`overwrite_enforces_physical_cap_atomically` storage test verify that a rejected
candidate is not published. Re-run both layers if the session adapter or
storage publication boundary changes.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-004-ext-proc-continue-and-replace.md`
- Interactive client: `edgion-gateway/src/plugins/http/ext_proc/client.rs`
- Mutation application: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`
- Request-body publication failure tests:
  `request_body_mutation_failure_replaces_final_success_summary_per_policy`
  and
  `request_body_mutation_failure_deactivates_pending_response_per_policy`
- User documentation:
  `docs/en/user-guide/http-route/filters/edgion-plugins/ext-proc.md`

## Resolution references

- `BODY-004` — 2026-07-27.
- `ext-proc-terminalize-owner-on-request-body-publication-failure` — request-body
  publication failure terminalizes the shared owner for both failure policies.
- `correct-ext-proc-phase-exchange-knowledge` — replaced the removed batch-
  exchange description and made the per-phase mutation boundary explicit.
