---
name: ext-proc-interactive-continue-and-replace
description: Use when reviewing ExtProc CONTINUE_AND_REPLACE semantics; covers interactive phase sending, stream half-close, and bounded response transport windows.
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

WP1 adds bounded response transport-window mutation, but response-header and
response-body `CONTINUE_AND_REPLACE` remain explicitly unsupported: their
stop-sending semantics are not equivalent to an ordinary per-window mutation.

## Core Rationale

**1. A headers verdict controls whether the original body is disclosed**

Submitting headers and body together makes `CONTINUE_AND_REPLACE` impossible:
the processor has already received the body it instructed the proxy to discard.
`ExtProcClient::exchange_all` now sends the next phase only after the current
phase has returned a matching `CONTINUE` verdict.

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

## Fix Suggestions Not Accepted

- Send all request phases before reading verdicts — exposes the original body
  before a headers replacement decision.
- Open a separate gRPC stream per headers/body phase — loses transaction state
  and is not the ExtProc bidirectional-stream contract.
- Leave the sender open until the verdict arrives — deadlocks processors that
  wait for the request stream's final EOF.
- Ignore response-header replacement bodies — silently claims support for a
  mutation Edgion cannot apply.

## Re-evaluation Triggers

Re-open the terminal-status decision only if:

- Direction-level response replacement/stop-sending semantics are specified.
- ExtProc adds another terminal response status.
- The client moves from ordered phases to explicit correlation identifiers.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-004-ext-proc-continue-and-replace.md`
- Interactive client: `edgion-gateway/src/plugins/http/ext_proc/client.rs`
- Mutation application: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`
- User documentation:
  `docs/en/user-guide/http-route/filters/edgion-plugins/ext-proc.md`

## Resolution references

- `BODY-004` — 2026-07-27.
