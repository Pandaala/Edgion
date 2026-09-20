# B0046 — Connection Rate Limit Plugin

## Scope

- Baseline: `76dd313272411be063b59f3af3debb393986b6eb`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- File: `edgion-gateway/src/plugins/stream/connection_rate_limit/plugin.rs`.
- Allowed code or knowledge files: none; analysis concluded examined-no-change.
- Forbidden files: every source/knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Exact commit allow-list: B0045/B0046 campaign ledger, batch, main, and changelog records only.

## Context and value gate

- Analysis verdict: `examined-no-change`, revalidated after B0044/B0045.
- Construction revalidates the schema and fails closed; runtime compilation also rejects invalid enabled entries before construction.
- Local listener admission runs first, followed deterministically by optional distributed listener and source-IP checks; distributed denial intentionally does not refund the already-consumed local token.
- Each Redis dimension freshly resolves the canonical LinkSys client, pins its Arc across health/check operations, uses the shared Redis rate-limiter core, and applies a validated bounded timeout and the grandfathered failure-policy shape.
- Source-IP keys use physical `direct_peer_ip`. B0044 now canonicalizes mapped IPv4 at listener admission, while TCP/UDP routes already canonicalized their physical peers, aligning all Stage-1 key spellings without trusting PROXY-derived identity.
- Metrics have bounded labels, per-connection denials remain in canonical access-log reasons, no request tracing is emitted, and immutable publication pins the compiled runtime generation.
- Current schema/controller/Gateway validation, documentation, regression knowledge, local/Redis scenarios, and failure semantics remain aligned.

## Implementation

- None. No material improvement justified a source, documentation, or knowledge edit.

## Independent review

- An independent reviewer approved the examined-no-change verdict with no findings after rechecking validation parity, layered admission order, Redis client/core ownership, timeouts, failure policy, IP-key composition, publication, and observability.

## Verification

- Analysis/current-HEAD revalidation and review passed the Gateway rate-limit suite with one measurement ignored, 88 stream tests with one ignored, 12 resource validation tests, 5 bridge tests, and Controller preparse parity.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only. Only the campaign's known lint and duplicate-dependency warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic records-only commit.
