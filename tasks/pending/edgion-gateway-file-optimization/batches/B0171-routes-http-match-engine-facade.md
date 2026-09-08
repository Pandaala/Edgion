# B0171 — HTTP Match Engine Module Boundary

## Scope

- Candidate analysis HEAD: `837bb26de18fc03ea97d4f9650a1756a79577c08`; freshness-revalidated and promoted unchanged at `bf9e618e9e755c56810730c919a608237e213cca`
- Pipeline state: terminal; changed
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/routes/http/match_engine/mod.rs`
- Baseline blob: `f72789157172de23a1467723d8e929e8903b8ca5`
- Exact technical implementation allow-list:
  - `edgion-gateway/src/routes/http/mod.rs`
  - `edgion-gateway/src/routes/http/match_engine/mod.rs`
  - `edgion-gateway/src/routes/http/match_engine/radix_path.rs`
  - `skills/01-architecture/02-gateway/03-routes/01-http-route.md`
  - `skills/04-review/routing/regression-guards/radix-candidate-precedence-rank-precomputed.md`

## Handoff

- Analysis: `b0168_final_review` read the facade, all re-exports/importers, matcher implementation types, tests and precedence knowledge, then revalidated after B0170.
- Decision: simple change. Restrict the parent matcher module to crate visibility, its three implementation children to the HTTP route subsystem, remove the unused regex-engine re-export, and update the knowledge page from convention to enforced internal boundary.
- Value: external code can no longer construct a partially initialized `RadixPath` before engine build computes precedence rank; no production consumer needs the public chain.
- Preserved behavior: all current sibling/descendant imports remain legal; matching, precedence, publication and request behavior do not change.
- Implementation: the parent matcher module is now crate-visible, its three implementation children are visible only to `routes::http`, and the unused regex-engine re-export is gone. The visibility change exposed three persistent `RadixPath` fields as production-dead; adjudication classified that warning as batch-caused, so the same batch removed `original`, `segment_count`, and stored `has_params` while retaining constructor-local parameter classification.
- Correction review: the independent reviewer raised the new dead-code warning as a Must Fix. A fresh adjudicator confirmed the attribution and limited the correction to dead-field removal rather than absorbing B0172's borrowing/allocation work. The original implementer applied the correction, and the independent reviewer returned LGTM with no remaining Must Fix, Should Fix, or deferred finding.
- Testing: matcher tests passed 39/39; the targeted Gateway check is warning-free. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff, and 3,113-file frozen-path guards passed. No integration gap or edit.
- Commit: `34e4ea7e`; never pushed by the campaign.
