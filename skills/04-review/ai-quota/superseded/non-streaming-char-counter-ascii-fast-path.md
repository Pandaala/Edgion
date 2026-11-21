---
name: non-streaming-char-counter-ascii-fast-path
description: Historical optimization for the retired raw response character counter.
knowledge_type: superseded
---

# Retired non-streaming character counter

This page records the former implementation. The accounting correction in
`gw-ai3-02` retired the counter and its tests. For current behavior see
[Chat output estimates](../regression-guards/chat-output-estimation.md).

## Historical implementation

`Utf8CharCounter::feed` counted raw characters in non-streaming Chat response
bodies. It returned an ASCII chunk's length when no partial UTF-8 prefix was
pending. Other chunks went through its byte loop because an invalid continuation
could require counting bytes from the previous chunk. A historical counter-only
probe measured roughly 35x faster counting for all-ASCII 1-16 KiB chunks.

## Historical verification

`char_counter_ascii_fast_path_matches_per_byte_loop` and
`char_counter_pending_prefix_bypasses_ascii_fast_path` covered partitioning,
malformed UTF-8 and pending-prefix recovery.
`cargo test -p edgion-gateway --lib accounting::usage` passed (54 tests) on
2026-11-07 against the former implementation.

## Resolution references

- `usage-01` (P3 performance) — fixed 2026-11-07.
