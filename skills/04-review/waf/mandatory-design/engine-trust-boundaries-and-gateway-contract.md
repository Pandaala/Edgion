---
name: engine-trust-boundaries-and-gateway-contract
description: Use when reviewing any edgion-waf change that reads operator rule sets, phrase assets, includes or client request bytes, or that changes what the crate promises the gateway (bounded CPU, bounded memory, no panic).
knowledge_type: mandatory-design
---

# WAF engine trust boundaries and the gateway contract

## Conclusion and scope

`edgion-waf` (a `#![forbid(unsafe_code)]` leaf crate, `edgion-waf/src/lib.rs`) consumes four input
surfaces with different trust levels. For every surface the crate promises the gateway three things:
**CPU bounded by input size and explicit limits, memory bounded by explicit limits, and no panic**. Errors are
returned as typed values (`CompilerError` / `SemanticError` / `RuleBundleError` at compile time,
`RuntimeError` on the request path) and the caller decides the response. This page owns that contract; the
mechanism-specific regression guards in `../regression-guards/` record how individual paths meet it.

| # | Surface | Trust | Entry points | Typical attack |
|---|---|---|---|---|
| 1 | Operator rule sets (SecLang source, `ruleSelection`, profiles) | semi-trusted: written by an operator, validated by the Controller | `compile_rule_bundles*`, `inspect_compatibility` (`seclang/include.rs`, `seclang/compiler/`) | compile-time CPU/memory exhaustion, limit-arithmetic overflow, regex blow-up, crafted directive graphs |
| 2 | Client requests (head, body, chunks, response head/window) | untrusted | `Transaction::*` (`seclang/transaction/`, `seclang/json_inspection_runtime.rs`, `seclang/xml_inspection_runtime.rs`) | decode-chain bypass, parser differentials, detector evasion, panics, integer overflow, exhaustion within limits |
| 3 | Phrase assets and includes (bundle members referenced by `@pmFromFile` / `Include`) | semi-trusted, same owner as 1 but resolved through a sink | `IncludeSink`, `BundleIncludes`, `BundleAssets` (`seclang/include.rs`), `PhraseAssetSink` (`seclang/compiler/mod.rs:219`) | include cycles and fan-out, oversized assets, quadratic dedupe |
| 4 | The gateway contract (how the host drives the engine) | trusted host, but the boundary itself is a contract | `edgion-gateway/src/plugins/http/waf/` | host relying on behavior the crate does not promise |

## Rationale and evidence

### Controls per surface

**1. Operator rule sets.** Every front-end stage has its own limit struct with defaults, and every loop over
operator-controlled data is charged before the work it pays for:
`SourceLimits` (`seclang/source.rs:25`), `TokenizerLimits` (`seclang/tokenizer.rs:19`), `SyntaxLimits`
(`seclang/syntax.rs:21`), `DirectiveLimits` (`seclang/directive.rs:16`), `TargetSyntaxLimits`
(`seclang/target.rs:14`), `OperatorSyntaxLimits` (`seclang/operator.rs:17`), `ActionSyntaxLimits`
(`seclang/action.rs:16`), `GraphLimits` / `FoldLimits` / `OperandLimits` / `SemanticLimits` /
`ValidationLimits` (`seclang/semantic/mod.rs:46,73,88,143,425`), `CompilerLimits`
(`seclang/compiler/mod.rs:71`), `MatcherLimits` / `RegexProgramLimits` (`matcher.rs:56,113`: nest limit, AST
and HIR node counts, multiplicative repetition work, `nfa_size_limit`, program peak) and `BuildLimits`
(`aho_corasick/mod.rs:90`). `FrontendLimits` (`seclang/include.rs:92`) aggregates them for one compile.
Counting and size arithmetic is `checked_*`; the canonical work counter is shared across two-pass
selection compiles.

**2. Client requests.** `RuntimeLimits` (`seclang/transaction/mod.rs:102`) bounds input bytes, headers,
query arguments, cookies, collection entries, value bytes, retained bytes, selected values, findings,
operator evaluations, operator work, transformation output/work, matcher scratch, TX variables/bytes,
setvar/ctl/capture counts and response windows. Sub-limits: `BodyLimits` (`seclang/body.rs:18`, including a
per-publication work budget), `TransformLimits` (`seclang/transform.rs:21`), `JsonInspectionLimits`
(`seclang/json_inspection_runtime.rs:15`), `JsonStreamLimits` (`seclang/json_stream.rs:19`),
`JsonStructureLimits` (`seclang/json_checker.rs:17`), `ResultLimits` (`seclang/json_inspection.rs:22`),
`XmlInspectionLimits` (`seclang/xml_inspection_runtime.rs:12`), `XmlStreamLimits` (`seclang/xml_stream.rs:8`)
and the detector input cap `MAX_INPUT_BYTES` (`injection.rs:10`). Matcher scratch is bounded up front:
`Transaction::new_with_handle` (`seclang/transaction/mod.rs:673`) sums the generation's scratch upper bound
and checks it against both `max_matcher_scratch_bytes` and `max_retained_bytes` before any request byte is
read; scratch itself is built lazily inside that bound. Ordinary operator work is charged per rule unit
(`charge_operator_work`, `seclang/transaction/selection.rs:649`); detector and ctl work are
transaction-wide counters (`detector_work_bytes`, `ctl_work_bytes`, `seclang/transaction/mod.rs:553-558`).

**3. Phrase assets and includes.** `IncludeLimits` (`seclang/include.rs:54`) bounds include depth, source
count, per-source and aggregate bytes, identity-comparison bytes and provenance; cycles are detected by
stack identity. Bundle member/entrypoint counts are checked before member validation and dedupe comparisons
are charged (commit `53229a191`, guard
[front-end dedupe and selector scans](../regression-guards/frontend-dedupe-and-selector-scans-are-charged.md)).
`PhraseAssetSink::extend_once` checks per-asset, cumulative and working-peak bytes before reserving;
`resolve_asset` / `normalize_asset` (`seclang/compiler/work.rs:324,397`) enforce request counts and
per-asset/total phrase counts and bytes. A missing optional member is a hard error, not a silent skip.

**4. The gateway contract.**
- *Fail-closed layering.* Request-path failures are `RuntimeError` values
  (`seclang/transaction/mod.rs:321`). The crate never chooses an HTTP status, never logs, and never
  degrades to "allow" on its own: a limit, invariant or parse failure ends the operation with an error, and
  the gateway maps it (`edgion-gateway/src/plugins/http/waf/mod.rs`, `WafErrorCode`). The one in-crate
  policy fork is `RuleEngineMode::DetectionOnly`, which turns a malformed body into a raw-only fallback
  publication instead of an error (`seclang/transaction/phases.rs:92-113`; consequences tracked in
  `tasks/improve-issues/waf5-01-*`).
- *No panic.* Request-path code returns `RuntimeError` for every reachable failure. `expect` /
  `unreachable!` are allowed only where the panic is logically impossible and the message states why
  (`skills/00-design-rules.md` section J); the current site-by-site classification is in the
  [2026-11 audit](../review-evidence/known-bypass-class-audit-2026-11.md). The crate does not contain
  panics: the **gateway owns the panic boundary** (section W of the design rules governs whether the host
  wraps the engine in `catch_unwind`; as of 2026-11-07 the WAF plugin does not, so a crate panic unwinds the
  Pingora worker task).
- *Body-processor selection.* The crate maps a `Content-Type` value to a processor
  (`processor_from_content_type`, `seclang/body.rs:235`; first `Content-Type` header wins) and honors
  `ctl:requestBodyProcessor`. Whether a body is buffered and published at all, in which inspection mode,
  and what happens to content types that fall through to RAW is **gateway policy**
  (`edgion-gateway/src/plugins/http/waf/mod.rs:485-490`); the CVE-2022-39955..39958 class is reviewed there.

### Invariants reviewers must hold

1. **Charge before work.** Every loop whose trip count or per-iteration bytes depend on operator- or
   client-controlled data is charged against a limit before the iteration runs, with checked arithmetic.
   A check placed after the loop, or a scan that runs before the first charge, is a defect even when
   another limit happens to bound it (precedents: F-S1-1..3 fixed in `53229a191`).
2. **No new request-path allocation outside accounted scratch.** Request-path storage is either borrowed
   from the request/generation, charged to `max_retained_bytes`, or lives in scratch whose upper bound is
   checked at `Transaction::new_with_handle`. Reusable scratch must not grow past its accounted bound.
3. **No lock and no blocking on the request path.** The compiled generation is immutable and shared; a
   transaction owns its mutable state. The only atomics are generation/identity counters
   (`aho_corasick/mod.rs:65`, `seclang/json_inspection.rs:7`), not request-path synchronization.
4. **Typed failure, never panic, never silent truncation.** A limit hit is an error variant naming the limit;
   it is never converted into "no match" inside the crate.
5. **Differentials are findings, not fixes by default.** A parser or transformation divergence from the
   pinned references (ModSecurity v3 / Coraza for SecLang semantics, libinjection-go v0.3.2 for
   `@detectSQLi` / `@detectXSS`, RFCs for parsers) is fixed in-crate only when the reference is unambiguous;
   otherwise it is recorded under `tasks/improve-issues/`.

Violation consequence: a single crafted rule set or request can stall a Controller compile or a gateway
worker, exhaust memory, or crash the worker, i.e. a denial of service for every route on that worker.

## Alternatives and limits

- Ordinary operator work is per rule unit, not per transaction; total request CPU is therefore
  `units x max_operator_work_bytes` in the worst case. That is a tracked gateway-level gap
  (`tasks/improve-issues/waf-02-*`), not a violation of this page, and the chosen per-unit scope is recorded
  in [chain ordinary work](../regression-guards/chain-ordinary-work-accumulates-per-unit.md).
- Operator rule sets are semi-trusted: compile-time limits protect Controller availability, not
  confidentiality. A rule set can still express a WAF that blocks nothing (see `waf4-02`).
- This page does not decide gateway-side behavior (status codes, inspection cutoffs, `catch_unwind`); those
  live in the gateway and in `waf-01`, `waf-05`, `waf-06`.

## Verification and re-evaluation

- Evidence for the current state: the five-lens audit in
  [known-bypass-class-audit-2026-11](../review-evidence/known-bypass-class-audit-2026-11.md) (2026-11-07,
  HEAD after `9ddd70ff9`), `cargo test -p edgion-waf`, the libinjection differential tests and the
  `randomized_byte_inputs_do_not_panic` test.
- Re-evaluate when: a new input surface is added (response body inspection, a new body processor, a new
  asset kind); a limit struct gains or loses a field; a request-path cache or shared mutable state is
  introduced; the regex engine or Aho-Corasick backend changes; the gateway adds or removes a panic boundary.

## Resolution references

- F-S5-1 (multipart panic, fixed `085fab687`), F-S1-1..3 (front-end charging, fixed `53229a191`), 2026-11-07
  second-pass security lenses S1-S5.
