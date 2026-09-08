# B0177 — HTTP Keepalive Timeout Conversion

## Scope

- Candidate analysis HEAD: `6df7f67fd7787463bcedd163f6d1bed2ce243db0`; freshness-revalidated and promoted unchanged at `0cd64e0d`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/http/proxy_http/parse_timeout.rs`
- Exact technical implementation allow-list: the owned file only

## Handoff

- Analysis: `b0168_final_review` read the complete timeout parser, shared GEP-2257 schema/parser, listener publication, client/backend consumers, route and gRPC deadline folding, pinned Pingora H1 keepalive behavior, tests, docs, and review precedent; it refreshed the listener-build chain after B0176.
- Decision: simple correctness fix. Preserve explicit zero but round every nonzero fractional-second keepalive duration upward to the next whole second before passing Pingora's seconds-only API.
- Value: `500ms` currently floors to zero and Pingora interprets `Some(0)` as infinite keepalive, reversing a finite operator bound. Ceiling preserves the configured upper-bound intent without shortening it.
- Preserved behavior: canonical parsing, exact-second/default values, explicit zero/infinite behavior, every other client/backend timeout, immutable publication, route/gRPC precedence, hot-path work, and observability remain unchanged.
- Implementation: a private keepalive-specific helper preserves zero, leaves exact seconds unchanged, and uses saturating ceiling for finite fractional durations. Only the client keepalive conversion uses it; canonical parsing and every other timeout remain unchanged. Four local tests cover zero, subsecond, exact/cross-second, maximum-duration saturation, config wiring, defaults, invalid fallbacks, and retained millisecond precision for other fields.
- Review: a fresh independent reviewer confirmed pinned Pingora zero/infinite semantics, ceiling and overflow behavior, cold publication, unchanged parser/fallback warnings and other timeouts, test quality, scope, and frozen coverage; verdict LGTM with no Must Fix, Should Fix, or deferred finding.
- Testing: conversion/config tests passed 4/4 and early-request-filter tests 4/4. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed. Existing integrations remain frozen and sufficient.
- Commit: `879bdbe0`; never pushed by the campaign.
