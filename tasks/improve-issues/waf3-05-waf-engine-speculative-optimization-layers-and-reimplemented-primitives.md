# waf3-05: The WAF engine carries a reimplemented Aho-Corasick crate, a private slab allocator and several proof-based optimization layers whose complexity is not justified by measured gains

- **Category**: over-design
- **Severity**: P3
- **Scope**: large-direction
- **Location**:
  - `edgion-waf/src/aho_corasick/` (≈4,240 lines: sparse/contiguous NFA, DFA, `Auto` selection, four prefilters, overlapping iterators, `stream.rs`, `replace.rs`)
  - `edgion-waf/src/aho_corasick/replace.rs` (308 lines: `replace_all*`, `try_stream_replace_all*` over `io::Read`/`io::Write` — no production caller)
  - `edgion-waf/tests/oracles/aho-corasick-1.1.5/` (differential oracle against the crate it replaces; `aho-corasick` is already in `Cargo.lock` via `regex`)
  - `edgion-waf/src/seclang/json_inspection.rs` (≈2,090 lines: generational slot `Pool<T>`, global `AtomicU64` owner IDs, a size-class `BytePool` free-list allocator over one `Vec<u8>`)
  - `edgion-waf/src/seclang/capture_hit_plan.rs` (optional batched "shadow" capture-hit matchers)
  - `edgion-waf/src/seclang/inspection_activation.rs` (static TX `skipAfter` gate proofs to prune JSON/XML predicate groups)
  - `edgion-waf/src/seclang/transaction.rs:2404-2491,2638-2741` (`json_summary_no_hit_tail`: hypothetical-capacity admission proof to skip candidate selection)
  - `edgion-waf/src/seclang/expensive_checks.rs` + `expensive_check_signatures.rs` (engine-embedded exact text of CRS 4.25.1 rules, including a 1.3 KB regex, used to decide which rules may be skipped)
  - `edgion-waf/src/lib.rs:15-18` (`#[allow(dead_code)]` on the whole `sqli` and `xss` modules)

## Summary
Beyond the SecLang language core, `edgion-waf` (~63k lines) contains a second tier of machinery whose
purpose is micro-optimization or speculative reuse: a full Aho-Corasick library (including an unused
string/stream replacement API) validated against the upstream crate it duplicates, a hand-written
memory allocator inside the JSON result store, and three independent "proof" layers that skip work only
when complex static or runtime admission conditions hold. Each layer adds its own invariants, error
states (`StoreError::Identity/Generation`, `PrivateInvariant`), budgets and regression guards. The
recorded evidence for these layers is component-level (bytes requested, call counts in debug builds); the
review knowledge explicitly states that none of it measures Gateway latency, throughput or RSS.

## Current behavior (evidence)
- Aho-Corasick: `aho_corasick/mod.rs` exposes `Builder` with `AutomatonKind::{SparseNfa, ContiguousNfa,
  Dfa, Auto}`, `AccelerationKind::{StartBytes, RareBytes, Packed, TwoGram}`, `MatchKind::{Standard,
  LeftmostFirst, LeftmostLongest}`, anchored/overlapping search, streaming and replacement. The WAF uses
  pattern-set scanning, leftmost search for capture phrases and the stream pattern-set path
  (`matcher.rs:1483-1505,2271-2285`); `replace.rs` has no caller outside its own module. The module
  header says it "does not depend on or wrap another matcher implementation", yet correctness is
  established by an oracle against `aho-corasick 1.1.5`. The former native-SIMD-search and DFA-state-layout tasks (now untracked per
  `skills/04-review/waf/SKILL.md`) targeted further work on this private copy.
- JSON result store: `Pool<T>` with `(owner, slot, generation)` IDs, `NEXT_STORE` atomic owner
  allocation, `BytePool` with 13 power-of-two classes plus page-rounded large classes and intrusive free
  lists written into the byte arena (`alloc`/`free`/`class_index`). This duplicates what `Vec`/the global
  allocator already provide, in exchange for capacity accounting that `try_reserve` plus a counter would
  give.
- Proof layers: `capture_hit_plan.rs` batches up to 32 capture regexes into extra matchers (guard page:
  +668,878 bytes of generation memory and +2.5 MB per-transaction scratch envelope); `inspection_activation`
  proves TX integer `skipAfter` gates stable across all phase-2 effects; `json_summary_no_hit_tail`
  simulates geometric `Vec` growth to prove a skip is admissible. Each has dedicated regression-guard
  prose in `skills/04-review/waf/regression-guards/fused-json-inspection.md` (≈660 lines).
- Exact-text CRS signatures (`expensive_check_signatures.rs`) couple engine behaviour to one CRS release:
  any upstream whitespace/regex change silently re-enables the rules (documented as conservative), and
  the mechanism is what makes `waf3-04` possible.

## Industry comparison
- ModSecurity v3 and Coraza use library regex/AC engines (PCRE2/Hyperscan, Go `regexp`, `ahocorasick`
  packages) and plain heap allocation per transaction; their performance work targets rule-set level
  wins (prefilters, `@pm` grouping, PCRE JIT), not per-request allocator replacement.
- Rust projects in the same space (e.g. `regex`, Vector, Linkerd) depend on `aho-corasick` directly; a
  private fork is normally justified only by a missing feature, recorded as such.

## Impact
- Very high maintenance and review cost: every rule-semantic change must be re-proven against several
  optimization layers (activation masks, summary tails, capture shadows, exact signatures), which already
  shows up as subtle semantic choices (e.g. `waf3-02` duplicate discard is entangled with the store's
  ownership model).
- Security-critical code paths have many conditional fast paths, increasing the surface for divergence
  between "Accelerated" and "Reference" evaluation.
- Duplicated primitives (AC library, allocator) must be kept correct and fast independently of upstream
  fixes.

## Suggested direction
1. Replace `edgion-waf/src/aho_corasick/` with the `aho-corasick` crate (already a transitive
   dependency); keep only a thin adapter for bounded pattern-set scanning. At minimum delete the unused
   `replace.rs` API and the unselected automaton/acceleration variants.
2. Replace the JSON `Pool`/`BytePool` with ordinary `Vec`/`Box<[u8]>` storage plus explicit byte
   counters; keep the logical budgets, drop the allocator.
3. Gate `capture_hit_plan`, `inspection_activation` pruning and `json_summary_no_hit_tail` behind
   end-to-end Gateway benchmarks (p50/p99 latency, RSS) on the supported CRS profile; remove any layer
   that does not show a material win.
4. Replace the exact-text `expensive_check_signatures` mechanism with either always-on execution or
   ordinary rule selection (tags/IDs) that operators can see.

## Notes
- `skills/04-review/waf/SKILL.md` now states that speculative optimizations are untracked and should be opened
  "only when measured profiling attributes a material cost to that area"; the existing layers predate that rule and their recorded evidence is
  explicitly non-Gateway. No intentional-design page justifies reimplementing Aho-Corasick
  (`skills/12-algorithms/SKILL.md` lists it as "not yet written").
- Out of scope: the SecLang parser/semantic pipeline itself, whose bounded design is appropriate for
  operator-supplied rule text.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P3 unchanged (large-direction maintenance item).
- `edgion-waf/src/aho_corasick/` is 4,240 lines. `replace.rs` is declared (`mod.rs:12`) but none of its replacement APIs are called outside that file. `aho-corasick` is already in `Cargo.lock`, and `tests/oracles/aho-corasick-1.1.5` exists. `lib.rs:15-18` marks the `sqli`/`xss` modules `#[allow(dead_code)]`. Total `src` is about 63k lines.
- `json_inspection.rs` (2,089 lines), `capture_hit_plan.rs` (522), `inspection_activation.rs` (360), and `expensive_check_signatures.rs` (366) all exist as described. `fused-json-inspection.md:637-650` confirms the evidence is debug-build allocator counts that "do not measure Gateway latency, throughput or RSS".
- Counterpoint to weigh before acting: the private AC exposes budgeted construction (`phrase_build_projection`, `BuildLimits`) for operator-supplied rule text, which the upstream crate does not offer. "Replace with the crate" therefore needs a thin build-cost bound on top, not a plain swap. Deleting `replace.rs` and the unused variants is uncontroversial.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P3 unchanged.
- `edgion-waf/src/aho_corasick/` (including the caller-less `replace.rs`), `json_inspection.rs` `Pool`/`BytePool`, `capture_hit_plan.rs`, `inspection_activation.rs` and `expensive_check_signatures.rs` are all unchanged.
- `skills/04-review/waf/SKILL.md` now marks SIMD/DFA/shared-body optimizations as untracked and to be opened only on measured profiling evidence; this supports rather than rebuts the finding. Stale `tasks/todo/` references in this file were updated.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
