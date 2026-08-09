# ExtProc — Envoy Parity Gaps Worth Closing

| Key | Value |
|-----|-------|
| Created | 2026-08-03 |
| Status | todo |
| Context | Follows the STREAMED request-body port (`tasks/archive/completed-tasks/request-body-capability-followups/08-ext-proc-streamed-request-body/`, review record `11-*.md`). Capability comparison vs Envoy `ext_proc` v3 done 2026-08-03. |

Current parity baseline: header phases, request body `BUFFERED` + `STREAMED`,
`ImmediateResponse` (incl. mid-body with header mutations), `failureModeAllow`,
TLS/mTLS to the processor, wire-compatible `envoy.service.ext_proc.v3` subset.
This task selects the gaps worth implementing, with rationale; everything else
is explicitly deferred at the bottom so it does not get re-litigated by default.

## Selected (in priority order)

### P1a — One `Process` stream across request AND response phases

Today the request side (headers + body chunks) shares one request-scoped
stream, but the response-header phase opens a separate short-lived stream, so
a processor cannot carry request-phase state into the response phase (the
single biggest behavioral divergence from Envoy for stateful processors:
request/response correlation, auth-context propagation, audit pairing).

Sketch: park the `ExtProcStream` (or a successor handle) in
`EdgionHttpContext` after the request phase (the `SyncWrapper` pattern from
`request_body_stream` applies — the handle is `!Sync`); the response-stage
`ExtProc` instance reuses it when present and configured for the same target.
Care points: two plugin ENTRIES (request vs upstreamResponse) are separate
instances built from separate configs — stream sharing must key on identical
`grpcService` config; fallback to today's short-lived stream otherwise.
Half-close discipline moves to response-phase completion / `pg_logging`.

### P1b — Honor `mode_override` from the header verdict

Envoy lets the processor's header response override per-request processing
modes (typical: "only stream me the body when the headers look suspicious").
Our flow already installs the streamed handler AFTER the header verdict, so
honoring `mode_override.request_body_mode` (NONE/SKIP ↔ STREAMED, and
BUFFERED where already configured) is a natural, cheap fit at
`run_request_streamed` / `run_request`. Guard: only allow narrowing or
switching between modes the operator's config admits (an operator who
configured NONE must not have a processor force-enable body streaming —
Envoy gates this with `allow_mode_override`; mirror that as a config knob,
default off).

### P2a — Headers-phase `CONTINUE_AND_REPLACE` under STREAMED

Envoy semantics: replace the whole body at the header phase and stop the body
direction. We currently reject it as a bad verdict (documented). This is
implementable on our stack and would be the FIRST consumer of the fork's
`UpstreamRequestBodyDisposition::Bodyless`: on CR, do not install a handler,
discard the client body (the pump drains it), and ship the replacement via
the bodyless replay-owner mechanism the BUFFERED path already uses for
bodyless CR. Verify the discard path against the fork's `Bodyless` fail-close
rule (real body bytes under Bodyless → 500), which likely means `Streamed`
disposition + an emptied chunk stream instead of true `Bodyless` — decide at
design time; this is the one open design question in this task.

### P2b — `override_message_timeout`

Processor-supplied per-message timeout extension, clamped to the operator's
`timeoutMs` ceiling (never above `MAX_TIMEOUT_MS`). Small, self-contained;
matters for processors with occasional slow verdicts (model inference).

### P3 (conditional) — `observability_mode`

Fire-and-forget: send phases without awaiting verdicts (no mutation, no
short-circuit). Valuable for audit/analytics processors at near-zero latency
cost. Needs careful semantics: no back-pressure, bounded send buffer with
drop-and-count on overflow (no unbounded queue — body invariant #3 spirit).
Pick up only when a concrete consumer exists.

## Explicitly deferred (do not pick up without a new decision)

- **Response body processing (any mode)** — blocked by the streaming-only
  response invariant (AGENTS.md HTTP body invariant #2). Requires the AI-0a
  `response_body_stream_filter` fork work first; that is its own workstream,
  tracked by the interface contract §5's closing note. Not an ExtProc task.
- **`FULL_DUPLEX_STREAMED`** — different back-pressure model (forward before
  verdict); large transport change, no consumer demand.
- **`BUFFERED_PARTIAL`** — niche; BUFFERED + physical caps cover the use.
- **Dynamic metadata / CEL attributes** — needs a metadata infrastructure that
  does not exist gateway-wide; revisit only with a cross-plugin design.
- **Request/response trailer phases** — the proxy never forwards request
  trailers (transport-level, contract §4.2); body-byte inspection is already
  complete without them.

## Ordering note

P1a and P2a both touch the stream lifecycle — do P1a first; P2a's design
question gets simpler once the stream handle lives in ctx.
