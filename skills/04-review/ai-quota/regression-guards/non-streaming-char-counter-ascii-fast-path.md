---
name: non-streaming-char-counter-ascii-fast-path
description: Use when reviewing the non-streaming AI output character counter (Utf8CharCounter::feed) and its ASCII fast path.
knowledge_type: regression-guards
---

# The non-streaming character counter's ASCII fast path is gated on an empty pending prefix

## Conclusion and scope

`Utf8CharCounter::feed` in `edgion-gateway/src/ai/accounting/usage.rs` counts the
output characters of non-streaming, non-Responses AI response bodies
(`UsageExtractionProcessor::observe_chunk`, `ResponseStreamKind::None`). When no
multibyte prefix is pending (`pending_len == 0`) and the whole chunk is ASCII, it
returns the chunk length directly instead of running the per-byte `feed_byte`
state machine. Every other chunk takes the unchanged byte loop. Counts, saturation,
invalid-byte recovery, `finish` idempotence and abort-without-EOS behavior are
identical to the byte loop. SSE counting, Responses accounting, the JSON usage
observer, reported usage and quota are not affected.

## Rationale and evidence

LLM JSON responses are almost entirely ASCII, and with no pending prefix
`feed_byte` returns 1 for an ASCII byte without touching any state, so the chunk
length is the exact result. `<[u8]>::is_ascii` replaces a branchy per-byte loop
on a request hot path; an exploratory counter-only probe measured roughly 35x
faster counting for all-ASCII 1-16 KiB chunks and unchanged cost for multibyte
chunks. End-to-end gain is smaller because the JSON usage observer still scans
the same chunk.

## Alternatives and limits

The `pending_len == 0` gate is mandatory. After an incomplete multibyte prefix,
an ASCII byte retroactively counts the now-invalid continuation bytes, so one
chunk's count can exceed its length (`\xf0\x9f\x98` then `A` counts 1 then 3).
Dropping the gate, or extending the shortcut to chunks that contain non-ASCII
bytes, undercounts.

## Verification and re-evaluation

`char_counter_ascii_fast_path_matches_per_byte_loop` compares `feed` with the
per-byte loop over every three-way split (empty chunks included) of ASCII,
permitted multibyte, truncated and malformed inputs, including per-chunk counts,
internal state and `finish`. `char_counter_pending_prefix_bypasses_ascii_fast_path`
pins the gate. Removing the gate makes both tests fail.
`cargo test -p edgion-gateway --lib accounting::usage` passed (54 tests) on
2026-11-07. Re-evaluate if `feed_byte` gains state that changes on ASCII input or
the counting definition changes.

## Resolution references

- `usage-01` (P3 performance) — fixed 2026-11-07.
