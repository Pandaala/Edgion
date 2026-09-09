# B0222 — Runtime TLS Matching

## Scope

- Pipeline state: analysis-ready; queued, not active mutation.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/matching/tls.rs`.
- Baseline: `00f896a2`.
- Analyst: `b0222_tls_analysis`.
- Read the full matcher, actual callback/configuration consumers and shared hostname/CA contracts. The adjacent test audit does not substitute for production-file value analysis.
- No writes authorized. Existing integration tests and user-owned handoff/WAF paths are protected. New integration requirements are deferred.

## Analysis Outcome

- Analyst recommends examined-no-change at `00f896a2`, target blob `692d7b6bd0c1af824c3b8cf938c2fe95f82c9e12`; source allow-list empty, no Cargo run.
- Full matcher, 26 tests, Gateway publication and early TLS consumers preserve ordered source proof, strict aggregate CA limits, rejection precedence, exact overlay attachment, RFC6125 wildcard depth, deterministic collisions and atomic snapshot ownership.
- Hoisting attachment proof still needs the same owned-string clones; merging entry literals is style-only. Shared validation/DER dedup redesign is not a local improvement. Winner-only collision publication is an unmeasured future memory candidate requiring ownership/promotion tests and a broader HashHost API, not a confirmed security defect or approved implementation.
- Extend the B0221 standalone knowledge follow-up with this file's obsolete fallback rustdoc and `multi-certificate-ref.md` / `runtime-expiry-recheck-symmetry.md` descriptions of removed APIs/per-handshake work. Do not change runtime to match stale text.
- At promotion independently review and freshly run `runtime::matching::tls::tests`; no integration changes.
