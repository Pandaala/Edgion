# B0173 — HTTP Regex Match Iteration

## Scope

- Candidate analysis HEAD: `34e4ea7eeae2aab01aca48179c1d0278e40ac631`; freshness-revalidated and promoted unchanged at `04c9c442623ff32196708e525c530e163a6f2d0c`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/http/match_engine/regex_routes_engine.rs`
- Baseline blob: `647c8f21ce5a8be2d4512c44cd05d5edd00ee1ab`
- Exact technical implementation allow-list:
  - `edgion-gateway/src/routes/http/match_engine/regex_routes_engine.rs`
  - `skills/01-architecture/02-gateway/03-routes/SKILL.md`
  - `skills/01-architecture/02-gateway/03-routes/01-http-route.md`

## Handoff

- Analysis: `b0168_final_review` read the complete regex build/request fallback path, radix sibling boundary, ordering and deep-match consumers, tests, routing knowledge, and upstream regex complexity contract; it refreshed the result after B0172.
- Decision: complex only because this tiny change touches the request-hot and precedence-sensitive fallback path. Iterate `RegexSet::matches` directly instead of collecting its indices into a second `Vec`, pin the relied-upon exact order in the existing multiple-match test, and correct three stale route-count-independent complexity statements.
- Value: each regex-fallback request avoids one secondary allocation and index copy. `RegexSet` still allocates its own result set, so the batch makes no allocation-free claim.
- Preserved behavior: radix-first fallback, stable regex precedence, first successful deep match, header/query/method handling, immutable publication, and all request results remain unchanged.
- Plan review: a fresh read-only reviewer verified the upstream ascending pattern-index iterator contract, zero-match and error semantics, exact three-pattern assertion, complexity correction, scope, and rollback plan; verdict Approved with no Must Fix.
- Implementation: `match_route` now iterates `SetMatches` directly and lets an empty iterator fall through. The existing multi-match test pins `specific`, `general`, then `wildcard`, and the source plus two routing pages now state the upstream worst-case `O(P * M)` bound without claiming allocation-free matching.
- Final review: a fresh independent reviewer confirmed iterator ownership, stable longest-first precedence, first deep-match/error propagation, documentation accuracy, the secondary allocation reduction, and exact scope; verdict LGTM with no Must Fix, Should Fix, or deferred finding.
- Testing: target tests passed 13/13 and the complete HTTP route suite passed 345/345. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed. Existing integration coverage is sufficient and remains frozen.
- Commit: pending; never push.
