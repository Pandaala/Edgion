# B0247 — HTTP Common Outbound Egress

## Scope

- Pipeline state: changed; committed locally.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `a9236e7579d09198c4c5315929103cfcfef0d9da`.
- Target: `edgion-gateway/src/plugins/http/common/outbound/egress.rs`.
- Target blob: `039723a88c9eac0aed35dbb635dc4fb28f9a4142`.
- Read scope may expand across the workspace to resolve caller, DNS, SSRF, redirect, proxy and final-dial ownership.
- Exact code allow-list: `edgion-gateway/src/plugins/http/common/outbound/egress.rs` and necessary canonical support `edgion-common/src/egress.rs`, including their inline unit tests.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; moderate cross-crate boundary, handled plan-first.
- Gateway startup accepts the complete `u64` millisecond range, while the canonical resolver computes `Instant + ttl` on the first hostname cache miss; an extreme accepted TTL can panic when the future instant is not representable. Gateway's general `Duration` adapter also narrows `as_millis()` with `as u64`, which can wrap.
- Store insertion time and compare saturating elapsed time against TTL. Preserve strict freshness (`elapsed < ttl`), immediate zero-TTL expiry, reversed-sample safety, bounded 16-key positive/negative cache behavior, DNS filtering, final-dial policy and observer ownership. Saturate Gateway duration publication to `u64::MAX` and reuse that value in its startup log.
- No integration edit or new deferred test issue is needed; pure unit tests can cover every arithmetic boundary. D0127 remains independent.
- Decisive knowledge: `skills/01-architecture/00-common/05-unification-points.md`, `skills/03-coding/03-time-conventions.md`, `skills/04-review/ssrf/mandatory-design/upstream-address-safety.md`, focused outbound SSRF regression guards, and unit-testing guidance.

## Outcome

- Gateway duration publication now saturates millisecond values at `u64::MAX` and uses the single converted value for both its atomic default and binary-owned startup log.
- The canonical common resolver stores insertion time rather than a possibly unrepresentable future `Instant`; lookup and pruning share `saturating_duration_since(inserted_at) < ttl`.
- Strict-boundary expiry, immediate zero-TTL expiry, reversed sample safety, huge-TTL freshness, positive/negative entries, the 16-key cap, lock/clone behavior, DNS filtering, SSRF observation and final-dial policy are preserved.
- Current blobs: Gateway target `4134e25a39d6ae164c8f50817f05b7c8d669fca6`; common support `7a200cd4668005c1440275d17496bb673c252343`.
- Independent plan review approved. Fresh final review returned LGTM with no Must Fix, Should Fix or deferred findings.
- Verification: common egress 8/8, Gateway egress 2/2, outbound client 30/30, OIDC 133 passed/8 ignored, LDAP 26/26 and ExtProc 102/102 (301 passed total). Workspace format/check/Clippy, Gateway build, agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed.
- No integration edit or new issue. D0127 remains independent.
- Commit: `98b88473`; never pushed.
