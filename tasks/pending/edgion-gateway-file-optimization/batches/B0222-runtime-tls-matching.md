# B0222 — Runtime TLS Matching

## Scope

- Pipeline state: terminal; local commit `b5e8156b`.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/matching/tls.rs`.
- Baseline: `00f896a2`.
- Analyst: `b0222_tls_analysis`.
- Read the full matcher, actual callback/configuration consumers and shared hostname/CA contracts. The adjacent test audit does not substitute for production-file value analysis.
- No writes authorized. Existing integration tests and user-owned handoff/WAF paths are protected. New integration requirements are deferred.

## Analysis Outcome

- Analyst recommends examined-no-change at `00f896a2`, target blob `692d7b6bd0c1af824c3b8cf938c2fe95f82c9e12`; source allow-list empty, no Cargo run.
- Full matcher, 26 tests, Gateway publication and early TLS consumers preserve ordered source proof, strict aggregate CA limits, rejection precedence, exact overlay attachment, RFC6125 wildcard depth, deterministic collisions and atomic snapshot ownership.
- Hoisting attachment proof still needs the same owned-string clones; merging entry literals is style-only. Shared validation/DER dedup redesign is not a local improvement. Winner-only collision publication is an unmeasured future memory candidate requiring ownership/promotion tests and evaluation of local builders before any shared HashHost API change, not a confirmed security defect or approved implementation.
- Extend the B0221 standalone knowledge follow-up with this file's obsolete fallback rustdoc and `multi-certificate-ref.md` / `runtime-expiry-recheck-symmetry.md` descriptions of removed APIs/per-handshake work. Do not change runtime to match stale text.
- At promotion independently review and freshly run `runtime::matching::tls::tests`; no integration changes.

## Promotion

- Original analyst refreshed at `72454f81`; target/callback/tests/shared helpers/projection types/TLS knowledge/rules unchanged. Empty technical source allow-list remains appropriate.
- D0124 already owns the knowledge corrections. Analyst explicitly classified collision-vector retention as a separate P3 investigation, D0125: snapshot maps retain full compiled candidates but lookup uses the first; the diagnostic cap is not an ownership cap. No measured urgency or security defect is claimed; local builder alternatives precede any proposed shared API change.
- Record-only commit scope: main task, ledger, changelog, B0221 SHA backfill, this record, queued B0224 record, coordinator deferred index/D0125 and standalone investigation task. Protected handoff/WAF and existing integration paths unchanged; freeze passed before delegation.

## Independent Review and Verification

- Fresh `b0222_final_review` LGTM. No Must Fix; its record-only wording correction was applied and rechecked so no shared HashHost API expansion is presumed. Full source/security ownership and D0125 investigation classification independently verified.
- Fresh TLS tests 26/26 passed in 0.22 seconds (`/tmp/edgion-b0222-tls.log`). Workspace format/check/Clippy (existing warnings), docs, SSA, metrics, Pingora source, Gateway tracing, English-only, diff and integration freeze passed.
- No runtime or integration changes; D0124 knowledge and D0125 measurement remain separate. Ledger/changelog updated; commit `b5e8156b`, local only.
