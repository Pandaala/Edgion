---
name: ext-proc-interactive-continue-and-replace
description: Use when reviewing ExtProc CONTINUE_AND_REPLACE semantics; covers interactive phase sending, stream half-close, and the response-body unsupported boundary.
status: fixed
finding_id: BODY-004
closed: 2026-07-27
---

# BODY-004 — ExtProc Interactive Continue-and-Replace — FIXED

## Conclusion

ExtProc phases on one direction are sent interactively on one bidirectional
gRPC stream. The gateway waits for the matching headers verdict before sending
the original body; `CONTINUE_AND_REPLACE` applies the valid request mutations
and stops that direction, while the final outbound phase immediately
half-closes the request stream.

Response-header `CONTINUE_AND_REPLACE` is explicitly unsupported until a
response-body replacement seam exists and therefore follows
`failureModeAllow`.

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

**4. Unsupported response replacement fails explicitly**

Edgion streams proxied upstream responses and has no response-body replacement
storage. Accepting response-header `CONTINUE_AND_REPLACE` while ignoring its
body mutation would be a silent protocol violation, so it is treated as an
unsupported bad verdict and routed through the configured failure policy.

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

Re-open this decision only if:

- Response-body replacement is implemented in the gateway.
- ExtProc adds another terminal response status.
- The client moves from ordered phases to explicit correlation identifiers.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-004-ext-proc-continue-and-replace.md`
- Interactive client: `edgion-gateway/src/plugins/http/ext_proc/client.rs`
- Mutation application: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`
- User documentation:
  `docs/en/user-guide/http-route/filters/edgion-plugins/ext-proc.md`
