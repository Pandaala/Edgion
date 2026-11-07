---
name: custom-terminal-101-normalized-before-dispatch
description: Review evidence for custom upstream terminal 101 normalization in the selected Pingora dependency.
knowledge_type: regression-guards
---

# Custom terminal 101 normalized before dispatch

## Recorded conclusion

Fixed in the local Pingora fork: a custom connector may report a completed Upgrade as `Header(101, true)`, which missed the `terminal_header` branch (101 is `is_informational()`) while still claiming the `TerminalBodyDispatch` latch — the response-body hook saw no EOS and the generic drain appended a plain `Body(None, true)` behind the 101, panicking the now-upgraded H1 writer. `custom_pipe_up_to_down_response` normalizes that shape to `Header(101, false)` + `Done` and stops reading the EOF'd session. Widening `terminal_header` to accept 101, or dispatching from `claim_for`, are not accepted.

## Current source verification

The pending `issue-custom-terminal-101-upgrade-panic` finding is already fixed
in Edgion's selected dependency: `Cargo.lock` resolves `edgion_v0.9.1` to
`e5f4287be713d01ffd444544251bf75c9d00062a`, re-verified by source inspection on
2026-11-06. At that revision `proxy_custom.rs` and `response_terminal.rs` are
byte-identical to `1aaa45cc`; `response_pipeline.rs` changed but still marks the
terminal state upgraded only after response filtering, and the four terminal-101
tests below are present. Historical: the original closure inspected `edgion_v4`
at `518f7274eb9fbd68f3f55e85b8c86a9e4a59e95e`, with identical files in the
sibling checkout `dba39b670b564fdded8af85a1f03f73fd6cb5379`.

The current producer `custom_pipe_up_to_down_response` normalizes terminal
101 to a nonterminal Header followed by Done, then returns without another
source read. `HttpTask::Header` carries the source EOS boolean;
`TerminalBodyDispatch.claimed` prevents repeated terminal dispatch, and
`upgraded` selects the emitted body variant. The shared `response_pipeline.rs`
now owns the consumer logic formerly in `custom_response_filter`: it marks
the terminal state upgraded only after response filtering, using the
downstream Upgrade request and the resulting 101 status. Thus a naked 101
or a response rewritten to 200 does not acquire the upgraded body variant
from its original status alone.

Existing tests in `pingora-proxy/tests/test_upstream_response_body_sink.rs`
cover terminal 101, naked terminal 101, rewritten terminal 101, and a
nonterminal 101 followed by Done. All four verify terminal output delivery;
the three terminal-header cases additionally assert single EOS dispatch and
no source read after EOF. This closure starts from source inspection, not a claim that historical
test runs were repeated. No runtime implementation change is needed.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

Canonical fork record: [http1/custom-terminal-101-normalized-before-dispatch.md](https://github.com/Pandaala/pingora/blob/518f7274eb9fbd68f3f55e85b8c86a9e4a59e95e/edgion-changes/review/http1/custom-terminal-101-normalized-before-dispatch.md).

Reopen if producer normalization is removed, a final filtered handshake no
longer owns the emitted body variant, or a terminal path loses or repeats EOS.

## Closure verification

On 2026-09-09, independent read-only review returned LGTM. Edgion's
`cargo fmt --all`, `cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` completed successfully; Clippy
reported warnings in unchanged code. The agent-doc validation, SSA force,
metrics inventory, and Pingora source policy guards also passed. Existing
Pingora regression tests were inspected, not rerun. This is an already-fixed
finding closure with no runtime code, resource field, or user-facing change.

## Resolution references

- `gw-16-review-04` — 2026-08-24.
- `issue-custom-terminal-101-upgrade-panic` — already fixed in the selected
  dependency; pending task revalidated on 2026-09-09.
