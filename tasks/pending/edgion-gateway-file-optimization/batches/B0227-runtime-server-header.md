# B0227 — Runtime Server Header

## Scope

- Pipeline state: verified changed (rustdoc only); awaiting local commit.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/server/server_header.rs`.
- Baseline: `415584d5`.
- Analyst: `b0227_server_header_analysis`.
- Read complete header policy construction and consumers, configuration/lifecycle ownership and protocol behavior before assessing local value.
- No writes authorized. Existing integration tests and user-owned external todo/handoff/WAF paths remain protected; new integration needs are deferred.

## Analysis and Promotion

- Analyst recommends one comment correction, no runtime optimization; target blob `fccf6940f46fd70a8893cb14e41965a08bcf81f0` at `415584d5`. No source/skills/rules changes through promotion `a63a88ca`; freeze passed.
- Exact technical allow-list: target `apply_to_response` rustdoc only. Locked http 1.5.0 defines HSTS as standard, so the per-apply custom-name allocation claim is false. Genuinely custom names may allocate. Preserve the separate Entry comment: Pingora's 12-entry title-case table really excludes HSTS.
- Full construction/schema/response/filter/error consumer analysis found pre-parsed fixed defaults, distinct sync/async ownership and meaningful remove/insert casing semantics. No runtime clone/container/flag rewrite or new issue justified; no operator configuration capability should be invented.
- Original knowledge: lifecycle architecture, coding/unit-test/review-routing rules, backend HSTS regression knowledge and local locked HTTP/Pingora implementation. Focused filters: provider-header-reset default restoration, fixed HTML replies, exact gRPC reply drain, gRPC-Web conflicting status-header regression.
- Coordinator scope: main task, ledger, changelog, B0226 SHA backfill, this record and subsequent queued analysis records. Protected handoff/WAF/integration paths unchanged.

## Implementation

- Separate implementer `b0227_implementation` corrected exactly three `apply_to_response` rustdoc lines after verifying locked HTTP name parsing and Pingora conversion/casing. Earlier Entry comment, executable code and tests unchanged. No plan-first gate needed for this simple comment-only correction.
- Fresh independent `b0227_final_review` checks the full target/consumers and original dependency evidence. Coordinator runs four existing response invariant filters and all required gates; no new integration work.

## Independent Review and Verification

- Final reviewer LGTM; no Must Fix/Should Fix. Three-line comment-only scope, standard HeaderName recognition versus separate Pingora title-case table independently verified; executable code/tests unchanged.
- Four focused filters each passed 1/1: provider-header reset, six fixed HTML responses, exact gRPC queued operations and gRPC-Web canonical trailer. Workspace format/check/Clippy (existing warnings), docs, SSA, metrics, Pingora source, Gateway tracing, English-only, diff and integration freeze passed.
- Logs: `/tmp/edgion-b0227-{provider,html,grpc,grpc-web,check,clippy}.log`. No new issue or integration change. Ledger/changelog updated; commit pending, local only.
