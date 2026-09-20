# B0052 — TLSRoute Stream Plugin Facade

## Scope

- Baseline: `b41a926ef82788260f19f2f01fc151ef5d19efad`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- File: `edgion-gateway/src/plugins/stream/tls_route/mod.rs`.
- Allowed code or knowledge files: none; analysis concluded examined-no-change.
- Forbidden files: every source/knowledge file, every existing `edgion-tests/**` file, unrelated modules, and protected user work.
- Exact commit allow-list: B0051/B0052 campaign ledger, batch, main, and changelog records only.

## Context and value gate

- Analysis verdict: `examined-no-change`, revalidated after B0050/B0051.
- The facade has one clear Stage-2 responsibility, accurate terminated/passthrough lifecycle documentation, and deliberate private-module/public-type exports.
- Canonical publication now flows through Store -> Publisher -> Generation; demanded Stage-2 immutable views are pinned for each admission, with no duplicate production publication path.
- Both TLS proxies invoke the shared dispatcher before upstream connect. Terminated mode carries negotiated TLS/mTLS identity; passthrough carries ClientHello/SNI context without pretending termination.
- `DirectPeerIp` and `RemoteIp` remain distinct and selected explicitly by `IpSource`; missing/malformed resources and Stage-1-only attachments fail closed.
- Empty, disabled, or bypassed Stage-2 declarations remain no-op without Stage-1 fallback. Outcomes remain in the canonical TLS access log with no hot-path tracing.
- Visibility tightening, public-runtime reshaping, facade tests, or lifecycle wording changes have no demonstrated value and would add API/testing churn.

## Implementation

- None. No material improvement justified a source, documentation, or knowledge edit.

## Independent review

- An independent reviewer approved the examined-no-change verdict with no findings after reconciling the facade, runtime/schema, Store -> Publisher -> Generation views, dispatcher, both proxy modes, identity, fail-close cases, and observability.

## Verification

- Current-HEAD revalidation and independent review passed 9 TLSRoute plugin and 10 serialized TLS route tests.
- Full repository gates passed: formatting, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only. Only the campaign's known lint and duplicate-dependency warnings remain.
- Final frozen-path verification passed for all 3,442 recorded integration paths.

## Handoff

- Ledger/changelog updated: yes.
- Commit: pending automatic records-only commit.
