# B0172 — HTTP Radix Build Ownership

## Scope

- Analysis and promotion HEAD: `34e4ea7eeae2aab01aca48179c1d0278e40ac631`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned files:
  - `edgion-gateway/src/routes/http/match_engine/radix_path.rs`
  - `edgion-gateway/src/routes/http/match_engine/radix_route_match.rs`
  - `edgion-gateway/src/routes/http/match_unit.rs`
- Exact technical implementation allow-list: the three owned files only

## Handoff

- Analysis: `b0169_final_review` refreshed the complete build, publication, request, precedence, test, and knowledge chain after B0171.
- Decision: simple change. Keep path extraction's zero-or-one contract but return a borrowed iterator, use borrowed map lookup and allocate an owned key only for a first unique normalized path, move each `RadixPath` into its bucket, and remove its obsolete `Clone` derive.
- Value: on every affected listener/hostname route-table rebuild, each non-regex candidate avoids a one-element `Vec` and three transient path buffers. The request path gains no new work.
- Preserved behavior: canonical request path, normalization, Exact/Prefix segment filtering, candidate grouping and order, precomputed rank, immutable publication, old-snapshot safety, and regex fallback remain unchanged.
- Implementation: `extract_paths` now yields its optional configured path by reference, radix construction queries the owned-key map through `&str`, materializes an owned key only for a first unique normalized path, and moves the compiled path into `radix_paths`. The obsolete `Clone` derive is gone.
- Review: fresh independent review verified iterator lifetime ownership, duplicate normalized-key bucket and path-index order, move-only storage, the concrete allocation reduction, immutable publication, and unchanged request/precedence/regex behavior; verdict LGTM with no Must Fix or Should Fix.
- Testing: matcher tests 39/39, match-unit tests 10/10, three conf-handler engine tests, and the duplicate-hostname build-order regression passed. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed. Existing Clippy warnings are unrelated. No integration gap or edit.
- Deferred: named parameter spelling currently affects path priority; the separate behavior-design task is `tasks/todo/http-route-named-parameter-precedence.md`.
- Commit: `04c9c442`; never pushed by the campaign.
