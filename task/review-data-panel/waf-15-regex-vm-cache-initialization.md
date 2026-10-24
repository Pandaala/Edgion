# Boolean regex VM cache initialization

Status: deferred ownership/performance decision for user review. No runtime change.
Source baseline: `82f3cd7f4`.

## Evidence

Gateway WAF transaction execution initializes each program's request-owned scratch
slot on first use through lazy_boolean_scratch or lazy_group_scratch. That outer
laziness is already implemented. Inside the reached slot, however,
CompiledRegexProgram::create_scratch always creates a PikeVM cache.

CompiledRegexProgram::is_match checks scratch ownership, then can resolve a Simple
execution hint without invoking the VM. A negative literal guard can also bypass
the VM for inputs at least 64 bytes long. CompiledMatcher::create_scratch similarly
creates its optional regex scratch before scan_inner applies accelerated exact or
guard paths. Grouped Reference scans deliberately bypass these hints and need the
VM, including when reusing scratch previously used by an accelerated scan.

The locked regex-automata 0.4.18 Cache::new initializes two ActiveStates owners,
whose reset sizes sparse sets and slot tables for the NFA. This is more than an
empty marker, although exact allocation counts and net request savings have not
been measured here. Capture regex execution has a separate scratch owner and is
outside this candidate.

## Decision alternatives

- Retain eager VM cache construction inside the first-use scratch wrapper if the
  simpler shape and common VM workloads outweigh bypass-only savings.
- Make the boolean wrapper's VM cache lazy, constructing it immediately before
  the first required VM call and retaining it for subsequent scans.
- Evaluate a specialized exact-hint scratch representation, with an explicit
  transition for Reference execution. Do not keep duplicate matcher logic merely
  to avoid representing that transition.

No alternative is selected. Measure first-use allocation/initialization savings
against wrapper layout, branch cost, VM-first workloads and repeated-use costs.
Include ordinary boolean programs, exact hints, guard misses/hits, grouped fanout
and alternating accelerated/Reference scans. Measure end-to-end SecLang execution,
not just isolated cache construction. No throughput, RSS or net allocation benefit
is established by this source audit.

## Frozen boundaries and acceptance

Owner validation must precede all exact/guard shortcuts; foreign scratch must
still fail. Preserve cleared physical result bits, logical fanout, source order,
empty predicates, byte semantics, optional-proof fallback and all capture/action
behavior. Reuse must initialize a needed VM cache once without sharing mutable
scratch across requests or generations. Reference execution must remain available.

Keep full-generation conservative scratch and aggregate retained admission, actual
Option capacities, error order and work charges. RegexCore::compile currently
accounts for the scratch wrapper, cache memory and first-use epsilon growth;
transaction accounting subtracts wrapper sizes from program envelopes and then
charges slot storage. Any layout change must reconcile both calculations, with
exact/one-less boundaries and first/repeated-use tests. Do not charge only reached
programs or treat conservative admitted bytes as measured heap usage. Retain the
validated NFA and all compilation/provenance checks.

This is distinct from [WAF-01](waf-01-prepared-matcher-scratch-budget.md), which
concerns preparing the generation-wide budget subtotal and explicitly excludes
engine-layout changes. It does not authorize that task or other deferred work.
Freeze the exact cross-crate allow-list and obtain independent IMPL plan/final
review before implementation. Add no schema, cache pool, body storage, unsafe code,
request logs or profiling metrics.

## Sources and verification

- `edgion-waf/src/matcher.rs`: RegexCore::compile, RegexBooleanScratch,
  CompiledRegexProgram::{create_scratch,is_match}, CompiledMatcher::{create_scratch,scan_inner}.
- `edgion-waf/src/seclang/transaction.rs`: lazy scratch helpers and scratch accounting.
- [Exact simple regex and demand-driven scratch contract](../../skills/04-review/waf/regression-guards/fused-json-inspection.md)
- `edgion-waf/tests/unit/matcher_test.rs`: exact/guard parity, owner, fanout and budget tests.

Fresh unchanged-source checks passed 46 matcher tests and the transaction lazy-slot
admission test. Two matcher release witnesses remained ignored. A focused simple
regex rerun passed four of those matcher tests, not four additional unique cases.
Formatting and workspace all-target check/Clippy passed with existing unrelated
warnings. No prototype, benchmark, full suite or live Gateway integration ran.
