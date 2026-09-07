---
name: h2-end-stream-wire-observer
description: Historical review evidence for Pingora fork / H2 END_STREAM wire observer; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 end stream wire observer

## Recorded conclusion

A GOAWAY's `last_stream_id` is retained as a persistent connection ceiling that later registrations are checked against, not a one-time `retain`. Malformed (declared payload < 8 octets), nonzero-stream, EOF-truncated, and `last_stream_id`-raising GOAWAY frames poison the observer permanently instead of contributing a guessed threshold. Clamping to 0, trusting a complete 4-octet id behind a truncated `error_code`, and keeping the ceiling in `FrameScanner` are not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-goaway-persistent-ceiling-fail-closed.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-goaway-persistent-ceiling-fail-closed.md)

## Resolution references

- `H2-005` — 2026-08-27 (fixed in pingora fork `edgion_v3`).
