---
name: custom-terminal-101-normalized-before-dispatch
description: Historical review evidence for Pingora / custom upstream terminal 101; revalidate against current code before applying.
knowledge_type: regression-guards
---

# Custom terminal 101 normalized before dispatch

## Recorded conclusion

Fixed in the local Pingora fork: a custom connector may report a completed Upgrade as `Header(101, true)`, which missed the `terminal_header` branch (101 is `is_informational()`) while still claiming the `TerminalBodyDispatch` latch — the response-body hook saw no EOS and the generic drain appended a plain `Body(None, true)` behind the 101, panicking the now-upgraded H1 writer. `custom_pipe_up_to_down_response` normalizes that shape to `Header(101, false)` + `Done` and stops reading the EOF'd session. Widening `terminal_header` to accept 101, or dispatching from `claim_for`, are not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [http1/custom-terminal-101-normalized-before-dispatch.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/http1/custom-terminal-101-normalized-before-dispatch.md)

## Resolution references

- `gw-16-review-04` — 2026-08-24.
