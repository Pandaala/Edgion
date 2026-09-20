# B0060 — Repeated XFF Field Lines

## Scope

- Baseline: `f3aa96144252ad1db949ecd73ddc376e176c3a6b`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Owned code file: `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs`.
- Allowed tests: inline tests in the owned file only.
- Existing `edgion-tests/**`, adjacent proxy/config/common files, knowledge pages, and protected user paths remain forbidden.
- Exact commit allow-list: the owned code file plus B0059/B0060 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Analysis verdict: `change`; implementation is plan-first because it changes trusted-proxy identity on the request hot path.
- `HeaderMap::get` currently retains only the first `X-Forwarded-For` field line for `RealIpExtractor`, aggregate byte-limit enforcement, request/access-log snapshotting, and upstream forwarding.
- Repeated field lines form one ordered logical XFF chain; truncation can leave an attacker-controlled earlier line as the only identity input when a trusted proxy appends another line.
- Build one bounded ordered XFF snapshot once, use it for canonical RealIp resolution when XFF is the configured source, store it in request metadata, and reuse it for access logging and upstream forwarding.
- Custom non-XFF RealIp headers retain the existing canonical extractor path.
- Preserve DirectPeerIp, ordinary absent/single-line behavior, duplicate/empty Host and path error precedence, invalid-input safe fallback, `XffHeaderTooLong`, and all body/plugin/route behavior.

## Plan requirements

1. Reuse `RealIpExtractor`; do not add ad-hoc IP parsing or another trust mechanism.
2. Walk all repeated XFF field lines in wire order, join valid lines exactly once with `, `, and enforce the configured aggregate cap with overflow-safe accounting.
3. Avoid allocation for missing input and retain the cheapest practical single-line path without borrowing beyond request headers.
4. Invalid non-ASCII values must fail safely to the effective downstream peer and never become a trusted identity or forwarded canonical chain.
5. Cover missing/single/repeated order, exact/over limit, trusted-proxy resolution, invalid values, metadata snapshot, and unchanged error precedence.

## Verification plan

- Focused request-filter and upstream-forwarded-header tests.
- Full campaign repository gates and frozen-path verification.

## Implementation

- Built one ordered, bounded XFF snapshot from every repeated field line, including empty values and separator bytes in aggregate-limit accounting.
- Reused that snapshot for canonical `RealIpExtractor` resolution, request/access-log metadata, and upstream forwarding while leaving custom RealIp headers unchanged.
- Invalid field bytes invalidate the whole logical chain without bypassing the cap; accepted requests fall back to the effective downstream peer and never forward the invalid value.
- Preserved duplicate-Host metadata timing and extracted the production validation-order seam so path, missing-Host, and XFF-size precedence is directly regression-tested.

## Independent reviews

- The independent plan reviewer approved the corrected plan after requiring preservation of duplicate-Host publication timing and raw aggregate accounting after invalid bytes.
- The first final review found no runtime defect but requested two stronger production-path regressions for validation ordering and invalid-under-limit forwarding.
- After those tests were added, the same independent reviewer returned LGTM with no Must Fix, Should Fix, or deferred findings.

## Verification

- All 26 focused request-filter tests passed, including eight XFF snapshot regressions.
- The canonical forwarded-header overwrite regression passed.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only.
- Only the campaign's known Kafka and frozen integration-test clippy warnings plus cargo-deny duplicate/advisory-ignore warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending; it will be backfilled by the next batch.

## Protected paths

- `task/`
- `tasks/pingora-0.9.0/`
- `tasks/todo/complete-k8s-dynamic-scenario-parity/`
- `waf-architecture-and-multi-pattern-matching.md`
