# SSE decoder — rebuild as a zero-copy byte-stream abstraction

| Key | Value |
|-----|-------|
| Created | 2026-08-05 |
| Status | todo — design discussion in progress, NOT ready to implement |
| Scope | `edgion-gateway` (`src/sse/`, `src/ai/usage.rs`, `src/ai/anthropic/response.rs`) |
| Source | Design discussion started from the D1 finding (`d1-sse-degradation-disables-quota-enforcement.md`) |
| Reference implementation | agentgateway (`crates/agentgateway/src/llm/policy/streaming_guardrails.rs`), NVIDIA NeMo Guardrails |

## Goal

Replace today's copy-based `SseDecoder` with a **zero-copy byte-stream
abstraction**: the decoder retains `Bytes` slices of the original transport
chunks instead of copying extracted values into owned `String`s, and an event is
represented as a **chain of slices** rather than a materialized string.

Decoding (UTF-8 validation, JSON escape decoding, JSON parsing) becomes a
**per-consumer opt-in** performed lazily on the slice chain, not an unconditional
cost paid by every event.

The motivating downstream capability is **response content auditing** (handing an
SSE window to a third-party moderation agent), which the current abstraction
cannot support at all because it discards everything except the extracted `data`
values.

## Why the current design is too heavy

Per event, today:

1. `pending.push(byte)` — copy 1, raw byte into the incomplete-line buffer
2. `cur_data.push_str(value)` — copy 2, line content into the accumulated data string
3. `serde_json::from_slice::<serde_json::Value>(...)` in the consumer — a full
   `Value` tree (a `Map` plus several `String`s and `Vec`s) allocated per event,
   just to answer "is `choices[].delta.content` non-empty"

Cost 3 dominates 1 and 2 by one to two orders of magnitude. Additional smaller
issues found while reading the code:

- `finish_line` uses `std::mem::take(&mut self.pending)`, which drops the buffer
  and forces a fresh allocation for every line. `reset_event` uses
  `pending.clear()` (capacity-preserving) for the same field. The `mem::take`
  site throws away capacity reuse for no reason.
- `process_byte` walks one byte at a time through a `match`, so line-boundary
  scanning cannot vectorize. A `memchr`-based scan is roughly an order of
  magnitude faster on that portion.

## Target shape

Bottom layer hands up a chunk → the decoder determines whether a complete SSE
event has formed → it consumes exactly the bytes forming that event → it yields
an `SseEvent` whose interior is **a chain of `Bytes` slices** that consumers can
operate on as a byte stream.

Consumers then choose their own decode depth:

| Consumer | Needs |
|---|---|
| Token counting | No JSON tree. A field-span scan plus escape-aware counting is enough. |
| Anthropic → OpenAI translation | Full semantic parse (it rewrites the payload). |
| Content auditing | Decoded text (escapes MUST be resolved — see Constraint 2). |

`SseEventRef<'a>` (a borrowed view) is viable because **`feed()` — the
`Vec<SseEvent>`-returning API — is test-only**; all production consumers use
`feed_each`, whose callback shape prevents the view from escaping. `feed()` can
simply be deleted.

## Verified facts (checked against source, do not re-derive)

### F1 — Pingora hands us right-sized `Bytes`, so slice pinning is a non-issue

`Bytes::slice()` / `split_to()` do **not** free the underlying allocation; they
share it by refcount. The concern was that retaining a small slice would pin a
whole large chunk. It does not, because:

- `pingora-core/src/protocols/http/v1/client.rs:458` —
  `Ok(read.map(Bytes::copy_from_slice))`. The H1 layer does keep a reusable
  64 KiB `BytesMut` (`BODY_BUFFER_SIZE`) and `read_body` returns a `BufRef` index
  into it, but `read_body_bytes` materializes a **fresh allocation sized exactly
  to the bytes read**.
- An incomplete SSE event is a **contiguous** byte run: once an event starts and
  has not terminated, every subsequent byte belongs to it. Only the chunk where
  the event *starts* can contribute a partial tail.

Therefore pinned memory ≈ retained bytes + at most one chunk of slack. The
existing `retained_event_bytes` accounting already counts **raw** line bytes
(`finish_line`: `pending.len() + 1`), not extracted bytes, so it already bounds
the raw quantity. **No copy-on-retain heuristic is needed** — only a one-chunk
slack allowance in the accounting.

### F2 — Escaped JSON is the common case, not an edge case

- `\n` appears in essentially every LLM answer (markdown, code blocks, lists).
- `\"` appears whenever the model emits code or quotes.
- `\uXXXX`: Python's `json.dumps` defaults to `ensure_ascii=True`, and many
  OpenAI-compatible servers are Python (vLLM, SGLang, FastChat, LiteLLM-style
  shims). **Non-ASCII content through a Python-based provider is essentially
  100% `\uXXXX`-escaped.** OpenAI's own API emits raw UTF-8, so there `\n` and
  `\"` dominate.

Impact is split:

- **Current token counting is unaffected** — a non-empty check reads a span
  length, and integers carry no escapes.
- **Auditing is hard-blocked by this** — running a regex over the raw span
  matches the escaped form, which for non-ASCII content is simply wrong.

Design requirement: separate **raw span** from **semantic value**, and resolve
escapes lazily, only for the consumer that needs semantics, and only over the
matched span.

### F3 — Cross-chunk discontinuity is the one genuine engineering problem

Independent of memory: a JSON field can straddle a chunk boundary, so a scanner
faces a physically discontinuous byte sequence. It cannot `memchr` directly and
cannot feed a regex engine directly.

Resolution: **cursor-based scanning over the slice chain, linearizing only the
matched value span** (which must be copied anyway to resolve escapes, so merge
the two steps). Linearizing the whole event defeats the purpose.

### F4 — Only two production consumers; the migration cost is concentrated

| Component | Lines | Migration cost |
|---|---|---|
| `src/sse/mod.rs` | 491 | Rewrite — this is the small part |
| `src/ai/usage.rs` (`UsageExtractionProcessor`) | 1220 | Small — 2 field-access sites |
| `src/ai/anthropic/response.rs` (`AnthropicResponseProcessor`) | 1640 | **Large — the real work** |

The Anthropic translator parses each event into a `serde_json::Value` and walks
it with `required_string` / `bounded_string`. Moving it to a borrowed or lazy
representation is a substantial rewrite. The decoder itself is not the bottleneck.

## Prior art — how the field actually does streaming auditing

All three do **hold-and-release with window accumulation**. None does true
per-event moderation. Streaming *parsing* and streaming *auditing* are separate
problems: you stream-parse to get events cheaply, then **re-accumulate a window**
to audit.

| | Mechanism | Window | Overlap | Verified |
|---|---|---|---|---|
| **agentgateway** | `pending_text: String` accumulates text deltas; `held_frames: Vec<Bytes>` holds raw frames **not yet forwarded**; evaluate on `pending_text >= eval_threshold`, `held_frames >= buffer_limit`, or EOF | `eval_threshold` **1024 bytes** (≈256 tokens) | `overlap_tail` **256 bytes**, prepended as `format!("{}{}", overlap_tail, batch)` | **Source-level** |
| **NVIDIA NeMo Guardrails** | Chunked validation with a sliding-window buffer of recent tokens | `chunk_size: 200` tokens | `context_size: 50` tokens | Vendor doc |
| **Kong AI Gateway** | Plugin runs per-frame, buffering each frame in memory; when enough data forms an "evaluable segment", it inspects that segment **before forwarding the frame** | Not documented | Not documented | Vendor doc |
| **Envoy AI Gateway** | ExtProc sidecar does token counting, transformation, and content moderation. Strong on **normalizing provider-side guardrail signals** (Gemini `SAFETY`/`BLOCKLIST` → OpenAI `content_filter`; Bedrock guardrail interventions returning HTTP 200 with no output) | No evidence of a response-side hold-and-release window | — | **Partial — response-side windowing NOT evidenced** |

Two independent implementations (agentgateway, NeMo) converged on **~200–256
token windows with ~25% overlap**. That parameter range is a usable starting
point; it does not need to be rediscovered.

NeMo states the tuning direction explicitly: larger chunks (200–256 tokens) give
better context for rails like hallucination detection; smaller chunks (128
tokens) lower latency but risk missing cross-chunk violations.

### What agentgateway does on violation

Held frames were never forwarded, so they are discarded and a synthetic SSE error
event is emitted: `{"error":{"type":"guardrail_blocked", ...}}`. Its source
records the accepted tradeoff: content flushed by **earlier passing windows**
cannot be retracted.

### Auditing forces a dual representation

Forwarding and auditing need two mutually non-substitutable forms of the same
data at the same time:

| Need | Required form | Why the other form cannot serve it |
|---|---|---|
| Forward to the client | Raw bytes, unmodified | Re-synthesizing SSE frames from reassembled text would lose `id` / `created` / `model` / `finish_reason` and would not match the upstream's framing boundaries |
| Send to an auditor | Contiguous, escape-resolved, readable text | An auditor cannot act on `{"delta":{"content":" Hel"}}` fragments |

agentgateway's two fields are exactly this split: `pending_text: String` is the
reassembled semantic stream for the auditor, `held_frames: Vec<Bytes>` is the
untouched raw stream for the client. It therefore holds **both** in memory for
the duration of a window. Kong's "buffer each frame, extract an evaluable
segment" is the same reassembly under different words. NeMo is not comparable
here — it sits in the LLM framework layer and sees text directly, with no SSE
frame layer at all.

### Reassembly is multi-stream, not a single concatenation

One SSE event sequence multiplexes several independent logical streams:

```json
"choices":[{"index":0,"delta":{
    "content":"...",                                   // main text stream
    "reasoning_content":"...",                         // reasoning stream (DeepSeek / o1-style)
    "tool_calls":[{"function":{"arguments":"..."}}]     // tool-argument stream, also fragmented
}}]
```

With `n > 1`, `choices[].index` values **interleave** in the same event
sequence. Reassembly must therefore bucket by **`(choice index, field)`**; a flat
concatenation would splice two different answers into garbage.

This also has an auditing consequence: the tool-argument stream should be
audited (prompt injection and data exfiltration both travel through it), but it
is a distinct semantic unit from the main text and must not share a window with
it.

### Forward-looking: multiplexed transports would need correlation-keyed routing

Today's assumption — **one SSE stream carries exactly one logical response** —
holds for `/v1/chat/completions` and Anthropic `/v1/messages`. One HTTP request
maps to one response stream, and although HTTP/2 multiplexes at the connection
level, each h2 stream is handled separately and the decoder is per-request, so it
never observes interleaving.

That assumption breaks for a class of protocols we do not proxy yet:

- **MCP over SSE** — one SSE connection is a server→client message channel
  carrying responses for **many concurrent requests**, correlated by JSON-RPC
  `id`. Genuinely interleaved.
- **A2A (Agent2Agent)** — same shape.
- **OpenAI Realtime over WebSocket** — event-multiplexed bidirectional channel;
  agentgateway explicitly extends guardrails to it.

This is why agentgateway is shaped the way it is: it is an MCP/A2A proxy first,
so connection-level multiplexing is its baseline assumption rather than an
extension.

If Edgion ever proxies these, a layer **above** `SseEvent` must route events by
correlation id rather than by stream position, and every per-stream piece of
state (audit window, overlap tail, token accumulator, degradation latch) becomes
**per-correlation-id** state with its own lifecycle and its own bound. Deciding
whether that is in scope changes the shape of this refactor, so it should be
settled before implementation rather than retrofitted.

### Where we can beat the reference implementation

agentgateway holds two representations concurrently (`pending_text` copied out
of the frames, plus `held_frames`). If our window accumulation operates directly
on the slice chain, the semantic stream need not be materialized at all until a
window is actually handed to an external auditor — one raw representation plus a
lazy cursor view bucketed by `(choice index, field)`.

Honest bound on the saving: escape resolution **necessarily** produces new bytes
(a six-character JSON unicode escape expands to its 2-byte UTF-8 form; a
two-character newline escape collapses to one byte), so the audit path must
materialize once at send time. What is saved is the per-event `String` append
and realloc growth, and the double residency of raw + text across the window —
a moderate win. The large win remains the **token-counting path**, which can
avoid materialization entirely.

## Constraints that will block review if not designed for

1. **HTTP body invariant #2** (`CLAUDE.md`) forbids a response-side
   `BodyStorage` and requires response-body plugins not to "accumulate unbounded
   state". A **bounded** window (N tokens / one sentence / a time window) does
   not violate "unbounded", but the invariant's wording must be updated to
   explicitly permit a bounded window, or the next reviewer will reject it.
2. **Escape resolution is mandatory for auditing** (F2). Raw-span matching is
   incorrect for non-ASCII content.
3. **Mid-stream termination already exists** — `terminate_over_quota`
   (`src/ai/usage.rs:59`) injects an SSE error event and terminates the stream.
   Quota termination and audit termination are the same action with different
   triggers. An audit capability MUST reuse this seam rather than build a second
   termination mechanism.
4. **A third-party auditor is an outbound call**, which triggers four
   cross-cutting rules at once: `build_outbound_client` (#1),
   `resolve_outbound_tls` (#2), `EgressResolver` (#3), and the **frozen failure
   policy** (#10 — a non-auth runtime dependency gets an explicit knob defaulting
   to close). The last one aligns naturally with agentgateway's
   `failureMode` (fail-open / fail-closed); no fifth failure shape is needed.
5. **No per-chunk logging or tracing** on the body hot path.

## The real tradeoff triangle for auditing

Holding frames before forwarding eliminates leakage entirely — this is what
agentgateway, NeMo and Kong all do. Leakage is a consequence of *choosing* to
forward first, not an inherent property of streaming.

The actual cost is **TTFT**. With a 256-token window at ~50 tokens/sec
generation, the first window takes ~5 seconds to accumulate before it can even be
evaluated, versus a ~500 ms TTFT without auditing — roughly a 10× first-token
regression. This is why NeMo offers `stream_first: True` (forward first, audit
after, with the explicit caveat that objectionable text may already have been
sent) and why agentgateway has `buffer_limit` as a second trigger alongside
`eval_threshold`.

So the triangle is **TTFT ↔ detection quality ↔ auditor API cost**. Leakage is
not one of the axes as long as hold-and-release is chosen.

## Scale reference (for sizing decisions)

| Unit | Size |
|---|---|
| One OpenAI SSE event, whole JSON envelope | ~250–350 bytes |
| Its `delta.content` payload | typically **1 token** (~4 bytes) |
| agentgateway audit window (1024 bytes text) | ≈256 tokens ≈ **~250 SSE events** |

The auditable unit is a semantic window, never a single event: PII / regex
patterns span ~3–15 tokens, harmful-content classification needs ~20–200 tokens,
and whole-answer policy checks need the full response.

## Proposed split

- **Task 1 (this file)** — the zero-copy byte-stream decoder. Pure
  infrastructure: slice chain, borrowed per-event view, lazy escape resolution,
  cursor-based cross-chunk scanning, `memchr` line scanning, `feed()` removal.
  No new outward capability.
- **Task 2 (to be split out)** — SSE content auditing: windowed hold-and-release,
  overlap, local regex layer, external auditor integration, reuse of
  `terminate_over_quota`, failure policy, outbound constraints.

**D1 is independent of both** and is being fixed narrowly first — it is a live
cost-control bypass and must not wait for this refactor. The decision it
encodes ("degradation must not silently disable quota enforcement") is a
requirement that carries into the new design regardless of implementation.

## Open questions

> To be filled in and worked through one at a time.

1. _(reserved — to be added)_

## References

- agentgateway `streaming_guardrails.rs` —
  `https://raw.githubusercontent.com/agentgateway/agentgateway/main/crates/agentgateway/src/llm/policy/streaming_guardrails.rs`
- agentgateway guardrails overview —
  `https://agentgateway.dev/docs/standalone/main/llm/prompt-guards/overview/`
- NVIDIA NeMo Guardrails output streaming —
  `https://developer.nvidia.com/blog/stream-smarter-and-safer-learn-how-nvidia-nemo-guardrails-enhance-llm-output-streaming/`
- Kong AI Gateway streaming —
  `https://developer.konghq.com/ai-gateway/streaming/`
- Envoy AI Gateway data plane —
  `https://aigateway.envoyproxy.io/docs/concepts/architecture/data-plane/`
