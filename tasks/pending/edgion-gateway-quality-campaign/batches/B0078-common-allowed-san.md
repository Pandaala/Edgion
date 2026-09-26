# B0078 — common-allowed-san

## Scope

- Baseline: `18e03b4c65b5225d0b299db55b8fb17c65232df5`.
- Pipeline state: terminal.
- Active anchor crate: `edgion-common`.
- Target: `edgion-common/src/allowed_san.rs`.
- Read supporting callers and tests as needed; no implementation scope is approved yet.
- Forbidden: existing `edgion-tests/**`, unrelated modules, and protected user task paths listed in the main campaign record.

## Context and value gate

Analyst: `common_coverage`. Outcome: defer. Read complete matcher and 13 unit tests, schema compiler and tests, RFC 6125 matcher, Controller validation, Gateway compilation and BoringSSL consumers. Typed identity, wildcard/IP behavior, display prefix separation and precompiled fail-close checks remain sound. Lowercase allocation changes have no demonstrated net benefit.

Confirmed: exposed ignoreCase is unused for RegularExpression by schema compilation and matching. D0016 tracks intended-contract resolution across schema/compiler/docs/tests. No code edit authorized; independent review pending.

Knowledge: TLS resource architecture/features, downstream TLS/mTLS, typed-SAN display guard, leaf-only whitelist guard, RFC6125 wildcard-depth guard, generation recompilation design, unit testing. Preserve all red gates, no handshake regex compilation/tracing, canonical TLS access logging and frozen integration files.


Promotion: `5339cb5ef`; intervening product changes are dependency classification and build-time OUT_DIR lookup only. Matcher/schema/TLS consumers and selected knowledge unchanged. Exact code allow-list: empty; records and D0016 task only.

Coverage also includes complete `edgion-common/tests/unit/allowed_san_test.rs`, read by analyst and required of reviewer; no test mutation.

## Independent review and verification

- Reviewer `b0078_review`: LGTM; full source/test and call-chain review confirmed defer classification. No Must Fix. Corrected the task method locator to AllowedSan::compile.
- Targeted matcher suite: 13 passed. No code or test mutation.
- Applied fmt; final fmt check, workspace all-target check/Clippy, docs, SSA, metrics, fork policy, transform contract, unit layout and Gateway tracing guard passed.
- Frozen integration checks passed; no protected user work staged.

## Exact commit allow-list

- `tasks/pending/edgion-gateway-quality-campaign/03-subtasks.md`
- `tasks/pending/edgion-gateway-quality-campaign/07-changelog.md`
- `tasks/pending/edgion-gateway-quality-campaign/edgion-gateway-quality-campaign.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0077-common-build.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0078-common-allowed-san.md`
- `tasks/pending/edgion-gateway-quality-campaign/batches/B0081-common-cert-duration.md`
- `tasks/pending/edgion-gateway-quality-campaign/deferred/D0016-allowed-san-regex-ignore-case.md`
- `tasks/todo/allowed-san-regex-ignore-case-contract.md`

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic commit.
