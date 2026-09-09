# B0244 — HTTP Common Facade

## Scope

- Pipeline state: changed; implementation, independent reviews and verification complete, awaiting local commit.
- Active anchor: `edgion-gateway`.
- Baseline: `8fc76e83`.
- Target: `edgion-gateway/src/plugins/http/common/mod.rs`.
- Target blob: `9c575602ae84d24e63d71b6a357a0ac358cb604f`.
- Inspect the complete facade, module visibility, exports, consumers, tests and relevant review precedent.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected.

## Analysis Outcome

- Decision: change, complex/plan-first. The default documented RateLimit and RateLimitRedis shape contains one `RemoteIp` source, while the shared request path currently allocates a temporary `Vec<Option<String>>`, then clones the owned `key_get` result into the composed output.
- Exact frozen implementation allow-list: `edgion-gateway/src/plugins/http/common/mod.rs` only. Special-case zero and one sources before allocating the multi-source vector: zero preserves the intentional empty global bucket, one transfers the owned extracted string, and two or more retain the existing injective positional encoding and extraction order.
- Preserve the three key-miss tags, default/Allow/Deny behavior, canonical `KeyGet`, sensitive-value handling, all caller output semantics and existing tests. Correct only the inaccurate `Resolved` documentation that excludes the valid empty global bucket.
- Existing colocated and consumer unit coverage is sufficient; no integration edit or deferred integration issue is needed.

## Outcome

- Added zero- and one-source fast paths to `resolve_rate_limit_key`: an empty list returns the intentional global-bucket key without extraction, and a single successful extraction transfers its owned `String` directly without the temporary vector and clone.
- Kept the two-or-more-source vector and injective positional encoding unchanged. All-missing sources still converge on the existing `defaultKey`, Allow and Deny branches with the exact existing log tags.
- Corrected the `Resolved` documentation for the valid empty key and strengthened the empty-list regression to prove zero `key_get` calls and no miss log.
- Independent plan reviewer `b0244_plan_review`: APPROVED. Independent final reviewer `b0244_final_review`: LGTM with no findings; confirmed exact bucket bytes, caller opacity and one-file technical scope.
- Verification: common 11/11, RateLimit-filtered 18/18, RateLimitRedis 14/14 and AI quota 23/23 test executions. Workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed.
- Commit: `654fe9a1`; never pushed by the campaign.
