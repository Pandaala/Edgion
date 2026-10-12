# Design R1

ARCH-020 separates discovery normalization from final executable assembly. The
private representation remains in `coordinator.rs`: `NormalizedBackendSlot`
contains a numeric `SocketAddr`, readiness, sorted merged originating proofs and
proof digest; `NormalizedServiceSection` contains checked port identity and a
slot vector; `NormalizedSectionOutcome` represents a section or typed invalid
outcome. Number/name aliases share section Arcs exactly as today.

`normalize_sections` and `normalize_section` replace provisional runtime builders.
Preflight and publication both call this same pure normalization (two passes are
retained). Counting unique normalized slots and deriving membership use that
representation. Content fingerprint byte order, section ordering, origin ordering,
proof domains and proof bytes remain unchanged. Published runtime helpers remain
for runtime consumers; they do not construct normalization.

Only `materialize_section` constructs fresh identity-bearing backend wrappers,
all/ready arrays, direct numeric index and all four eager selectors.
`materialize_sections` converts invalid outcomes and remaps aliases once per
unique section. A changed Service rematerializes every resolved section, including
empty or individually unchanged sections. Only transport counters are reused by
proof/socket; endpoint identity and eligibility state are fresh. Exact sorted
epoch reservation, zero-demand no-ops/deletes at exhaustion, all-or-nothing source
assignment, failure cleanup, joint publication and activation gaps are unchanged.

At the actual `CompiledSelectors::build` cold seam, test-only thread-local
construction instrumentation counts selector sets. Synchronous coordinator tests
compare thread-local count deltas: preflight and semantic no-op zero; one aliased
resolved final section one; changed two-section Service two; invalid-only and
failed allocation zero; an empty resolved final section still one. A terminal
epoch regression covers valid aliases plus invalid outcomes and exact demand.
Existing tests retain their identities and coverage.

## Frozen allow-list

- `edgion-gateway/src/backends/discovery/coordinator.rs`
- `edgion-gateway/src/lb/compiled_selector.rs`
- `edgion-gateway/tests/unit/backends/discovery/coordinator_test.rs`
- `skills/01-architecture/02-gateway/07-backends.md`
- `skills/04-review/backends/regression-guards/named-port-resolve-against-originating-slice.md`
- `skills/04-review/backends/regression-guards/ep-slice-no-port-fail-closed.md`
- `skills/04-review/backends/regression-guards/discovery-normalized-geometry-ownership.md`
- `skills/04-review/backends/SKILL.md`
- Task records within `tasks/pending/architecture-discovery-normalized-geometry/`

The original finding and review batch are read-only. Preexisting ARCH-016 dirty
files remain untouched, including load-balancing architecture and index pages.
Rollback restores only this task's own changes against its recorded baseline.

## Gates and verification

English-only disk content; no new logging, request observability, metrics, secret
surfaces, schemas, dependencies or persistent stores. Binary logging/access-log
ownership, sensitive redaction, observability restraint and canonical unification
remain unchanged. No new request-path allocation, lock or lazy compilation.

Run coordinator and compiled-selector tests, then broader backend and LB tests
under supported Gateway defaults. Root runs required formatting, workspace check,
Clippy, documentation and relevant policy guards after the mutation lane finishes.
Independent plan approval precedes production edits; independent final review
checks the code and maintained knowledge against original project rules.

## Baseline SHA-256

| Source | SHA-256 |
|---|---|
| coordinator.rs | `1869a0e26ecb55c60a4c9535be48ae2db1774e2b64c9f885f99533a1236b91f4` |
| compiled_selector.rs | `7d8e4fd31fa19bb1efe68c2c00a6a4074248cdbe7f657837aba719009fdb2a2f` |
| coordinator_test.rs | `4660e36d90e747ab3536bde49464a2dc111aabe7cf161bb5f1922af3dacf3f74` |
| 07-backends.md | `cbbe12b7dd9ce629cdbd8afe02ef98e37b0f50e7397df049429c1560c16d6096` |
| named-port-resolve-against-originating-slice.md | `e6de87d8d80ef970dcbc3c5cc36dce29a160d5a1b2893368a7060d23d2adafe3` |
| ep-slice-no-port-fail-closed.md | `4aac527a6d3e9ce13f938d87a7c72b6165a5c4b0fd95d4c797c7bcc53303e2ba` |
| backends/SKILL.md | `08b91977e02ddf2ad6b95b951a8738062065838d6dcadcf827f8af91bb848325` |
