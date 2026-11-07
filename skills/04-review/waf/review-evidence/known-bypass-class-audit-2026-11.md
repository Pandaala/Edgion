---
name: known-bypass-class-audit-2026-11
description: Use when reviewing edgion-waf against known WAF bypass and denial-of-service classes, when re-triaging a parser, transformation, detector or panic finding, or when a change adds a transformation, body processor, operator or regex engine.
knowledge_type: review-evidence
---

# Known bypass-class audit of the WAF engine (2026-11)

## Conclusion and scope

On 2026-11-07 (branch `feature-10-28`, series base `5aab0fd38`, table re-derived at the commit that adds
this page) `edgion-waf` was audited against the known WAF bypass and DoS classes below. One request-path
panic and four reference divergences were fixed in-crate; the remaining in-crate divergences are tracked
as `tasks/improve-issues/` files; gateway-owned classes are listed as out of crate. This page is evidence
for that point in time, not a standing guarantee. The contract it checks is
[engine trust boundaries and the gateway contract](../mandatory-design/engine-trust-boundaries-and-gateway-contract.md).

## Rationale and evidence

### (a) Method

Five read-only lenses, each against current source plus pinned references:

| Lens | Focus | Corpus / tools |
|---|---|---|
| S1 | malicious rule sets: compile-time exhaustion, limit arithmetic, front-end `expect` | source reading of `seclang/{include,compiler,semantic,source,tokenizer,directive,action,operator,target,tag_selection}.rs`, `matcher.rs`, `aho_corasick/`; regex-automata 0.4.18 `Builder::add` checked for incremental size limits |
| S2 | transformation chains and request-head projection | ModSecurity v3 / Coraza transformation semantics; OWASP CRS 4.25.1 rules tree (transformation inventory: all 20 `t:` names CRS uses are implemented; unimplemented names fail closed with `UnknownTransformation`) |
| S3 | body processors and parser differentials | urlencoded, multipart, JSON, XML parsers; existing chunk-split differential tests |
| S4 | `@detectSQLi` / `@detectXSS` and operator semantics | differential against libinjection-go v0.3.2 (pinned commit in `edgion-waf/INJECTION.md`): 600 000 generated inputs (200 000 SQL-shaped with per-character case flips, 200 000 XSS-shaped, 100 000 mixed, 100 000 uniform random bytes) plus 293 hand-written probes; 37 adversarial 4 MiB inputs for work bounds |
| S5 | panic reachability and integer arithmetic on the request path | every `expect` / `unreachable!` / `panic!` site plus direct indexing on request-derived indices; a PoC harness outside the tree (path dependency on the crate) confirmed the one reachable panic |

Fix policy: a divergence was fixed in-crate only when the reference is unambiguous (libinjection-go v0.3.2,
ModSecurity v3 and Coraza agreeing, or an RFC) and the change only makes behavior equal to it; every fix
commit carries a red run. Everything else became a finding file.

### (b) Class table

| Class | Status | Evidence |
|---|---|---|
| Body-processor selection by `Content-Type` (CVE-2022-39955/39956/39957/39958) | **out of crate** | The gateway decides whether and how a body is published (`edgion-gateway/src/plugins/http/waf/mod.rs:485-490`). In-crate mapping only: `processor_from_content_type` (`edgion-waf/src/seclang/body.rs:235`) uses the media type before `;`, ASCII-trimmed and case-insensitive, `+json`/`+xml` suffixes; the first `Content-Type` header wins; `ctl:requestBodyProcessor` overrides Auto. |
| Multipart malformations (`MULTIPART_STRICT_ERROR` family, Coraza multipart advisories) | **fixed in this pass** (panic) + **finding** + covered | F-S5-1 boundary adjacent to the previous boundary panicked (`&input[part_start..part_end]` with `part_end < part_start`): fixed `085fab687`, guard [multipart boundary search](../regression-guards/multipart-boundary-search-bounded.md), test `body_test.rs::multipart_boundary_directly_after_boundary_is_malformed_not_panic`. Covered: linear boundary discovery, 200-byte boundary cap, folding / missing name / missing final boundary fail closed in `On`. Findings: [waf5-01](../../../../tasks/improve-issues/waf5-01-detectiononly-malformed-body-drops-structured-inspection.md), [waf5-02](../../../../tasks/improve-issues/waf5-02-no-reqbody-error-variable-family.md), [waf5-04](../../../../tasks/improve-issues/waf5-04-multipart-content-disposition-parsing-divergences.md). |
| JSON depth, duplicates, numbers, UTF-8 (CVE-2021-42717, waf3-02) | covered + **finding** | Covered: depth checked against `max_depth` before push, lone/unpaired surrogates and invalid UTF-8 fail closed, control characters, trailing garbage, leading zeros/hex/`+` rejected (`json_stream_test.rs::every_byte_split_matches_serde_json_acceptance_and_whole_input_events`, `::zero_depth_allows_root_containers_but_not_nested_values`, `::syntax_escape_utf8_number_and_incomplete_errors_are_typed_and_sticky`). Findings: [waf3-02](../../../../tasks/improve-issues/waf3-02-json-duplicate-keys-drop-earlier-values-uninspected.md) (duplicate keys, behavior decision pending), [waf5-03](../../../../tasks/improve-issues/waf5-03-json-non-finite-numbers-rejected-as-malformed.md) (`1e999`). |
| XML entities, CDATA, comments, attributes (XXE, waf3-03) | covered + **finding** | XXE impossible: `<!DOCTYPE` / `<!ENTITY` fail closed, only the five predefined and numeric references decode, no IO (`body_test.rs::xml_publishes_text_and_attributes_and_rejects_dtds`, `::malformed_xml_nesting_and_unknown_entities_fail_closed`). Finding: [waf3-03](../../../../tasks/improve-issues/waf3-03-xml-text-split-at-comments-and-cdata-enables-payload-splitting.md) (text split at comment/CDATA/PI, behavior decision pending). |
| Transformations (waf-03; transformation-work DoS such as CVE-2023-38285) | **fixed in this pass** + **finding** + covered | waf-03 / F-S2-1 `htmlEntityDecode` without `;` and case-insensitive names: fixed `2ac1fc9c8`, guard [htmlEntityDecode reference semantics](../regression-guards/html-entity-decode-reference-semantics.md). Work: every chain is bounded by `TransformLimits` (`seclang/transform.rs:21`, checks in `walk_transformations` `:142`, `:205`) and charged to the unit budget. Identical to the reference: chain order, `t:none`, multiMatch stage-on-change, jsDecode, escapeSeqDecode, cmdLine, normalizePath(Win), removeCommentsChar, replaceComments. Findings: [waf5-05](../../../../tasks/improve-issues/waf5-05-transform-low-divergences-utf8tounicode-whitespace-cssdecode.md), [waf5-06](../../../../tasks/improve-issues/waf5-06-html-entity-decode-one-byte-truncation-divergence.md). |
| Request line / filename confusion (CVE-2024-1019, CVE-2024-46292) | covered | `request_filename` / `query_part` (`seclang/transaction/state.rs:576-580`) split the URI on the first literal `?` before percent-decoding, and the gateway normalizer never decodes `%3F`/`%23`/`%25`; `REQUEST_BASENAME` splits on `/` or `\`. Design note (not a finding here): `REQUEST_URI` is the gateway-normalized current URI; CRS 930100/930110 target `REQUEST_URI_RAW`. |
| Target selector semantics | **finding** | [waf5-07](../../../../tasks/improve-issues/waf5-07-header-name-regex-selector-case-sensitive.md): header-collection regex selectors are case-sensitive while literal ones are not. |
| Numeric operators and setvar arithmetic (waf3-01) | **finding** | [waf3-01](../../../../tasks/improve-issues/waf3-01-integer-operators-abort-transaction-on-client-input.md): non-i64 operand or i64 overflow aborts the transaction (`RuntimeError::IntegerConversion`); behavior decision pending. The setvar overflow sub-case (F-S5-2) is part of it. |
| Regex matching cost (CVE-2020-15598 class) | covered + **fixed in this pass** | Matching uses regex-automata (no backtracking; linear in input per program). Compile time: nest limit, AST/HIR counts, class-range projection, `nfa_size_limit`, program peak (`matcher.rs`). F-S1-3 additive nested-repetition estimate: fixed `53229a191` (multiplicative, `matcher_test.rs::nested_counted_repetition_work_is_multiplicative`, `::nested_counted_repetition_is_rejected_before_the_nfa_build`), guard [front-end charging](../regression-guards/frontend-dedupe-and-selector-scans-are-charged.md). |
| Detector divergence (`@detectSQLi` / `@detectXSS`) | **fixed in this pass** + covered | waf4-01 / F-S4-1 lower-case `INTO OUTFILE` whitelisted: fixed `5dacb60b7`, guard [native injection operators](../regression-guards/native-injection-operators-use-bounded-detection.md), test `sqli/mod_test.rs::three_token_into_outfile_whitelist_is_case_insensitive`. Same commit documents F-S4-2 (backslash-escaped quote follows C, Go v0.3.2 false positive) and F-S4-3 (Go v0.3.2 CDATA panic not reproduced) in `INJECTION.md` and `libinjection_diff.rs::known_oracle_divergences_follow_libinjection_c`. After the fix the S4 corpus leaves only the documented F-S4-2 class. Work bound: no super-linear shape among 37 adversarial 4 MiB inputs (0.08-0.52 s each). |
| Panics and `expect` (design rules J and W) | **fixed in this pass** + covered | F-S5-1 (above). All other sites UNREACHABLE; see table (c). Request-path parsers and transformations contain no `expect` / `unreachable!`; direct indexing on request-derived indices was checked site by site. Compile-time sites whose message did not state the invariant were reworded in the commit that adds this page. |
| Integer overflow | covered | Counting/size arithmetic is `checked_*` / `saturating_*`; percent-decoded length projection and copy share one admission rule; JSON number, surrogate, depth and XML depth arithmetic checked; `ip_matches` shift guarded; transformation `as u8` casts are intentional low-byte reference semantics. |
| Count x size limit bypass | covered + **fixed in this pass** | F-S1-1 (bundle dedupe post-paid) and F-S1-2 (forward selector scan before the first charge): fixed `53229a191`. Per-unit operator budget allows `units x 64 MiB` per request: out of crate (gateway-level, `waf-02`). Matcher scratch is bounded at transaction creation by both `max_matcher_scratch_bytes` and `max_retained_bytes` (the latter is the effective ceiling with defaults). |
| Gateway-side WAF behavior | **out of crate** | `waf-01` (inspection cutoff skips remaining phases), `waf-02` (no per-request CPU bound), `waf-05` (client-caused failures return 500), `waf-06` (generation not shared across plugins), `waf3-04` (expensive checks default off disables a PL1 multipart charset rule). |

### (c) Panic-site classification

Command (run at the commit that adds this page):

```
rg -n 'expect\(|unreachable!|panic!' edgion-waf/src --glob '!**/generated_data.rs'
```

It prints **126** lines; the table has **126** rows, one per line. 8 rows are `#[expect(...)]` lint
attributes that the pattern also matches (not panic sites). Of the 118 panic sites, 6 are on the request
path and 112 run only at compile time on operator-supplied SecLang; all 118 are UNREACHABLE and every
message states the invariant. Verdicts come from lens S5 (and S1 for the front end), re-mapped to the
post-split module paths. `src/` contains no `panic!`, no bare `.unwrap()` and no `unwrap_unchecked`.

| path:line (under `edgion-waf/src/`) | phase | verdict | message states why |
|---|---|---|---|
| `aho_corasick/mod.rs:375` | request-path | UNREACHABLE: every request-path automaton keeps the default unanchored start kind and the search input is unanchored, so `validate_input` cannot fail | y |
| `aho_corasick/mod.rs:384` | request-path | UNREACHABLE: every request-path automaton keeps the default unanchored start kind and the search input is unanchored, so `validate_input` cannot fail | y |
| `aho_corasick/mod.rs:397` | request-path | UNREACHABLE: every request-path automaton keeps the default unanchored start kind and the search input is unanchored, so `validate_input` cannot fail | y |
| `aho_corasick/prefilter.rs:214` | request-path | UNREACHABLE: the SWAR zero-byte test is exact (no false positives), so a scalar scan of the same 8-byte chunk finds the byte | y |
| `aho_corasick/nfa/sparse.rs:77` | compile-time | UNREACHABLE: the build graph always has root state 0 | y |
| `seclang/compiler/work.rs:154` | compile-time | UNREACHABLE: a nonzero template count implies a recorded source location | y |
| `seclang/compiler/work.rs:162` | compile-time | UNREACHABLE: a nonzero segment count implies a recorded source location | y |
| `seclang/compiler/work.rs:570` | compile-time | UNREACHABLE: every validated member retains its execution node | y |
| `seclang/compiler/work.rs:600` | compile-time | UNREACHABLE: the runtime charge is added exactly once per member in `prepare` | y |
| `seclang/compiler/canonical.rs:502` | compile-time | UNREACHABLE: inner match over the discriminant already narrowed to Literal/Path by the enclosing arm | y |
| `seclang/compiler/publish.rs:341` | compile-time | UNREACHABLE: tag ranges were pushed from compiled literals into `bytes` in the same function | y |
| `seclang/compiler/publish.rs:342` | compile-time | UNREACHABLE: tag ranges were pushed from compiled literals into `bytes` in the same function | y |
| `seclang/compiler/operator_plan.rs:384` | compile-time | UNREACHABLE: working-record charge is bounded by `CompilerLimits` | y |
| `seclang/compiler/operator_plan.rs:388` | compile-time | UNREACHABLE: `checked_sub` of records retained earlier | y |
| `seclang/compiler/operator_plan.rs:395` | compile-time | UNREACHABLE: `checked_sub` of bytes retained earlier | y |
| `seclang/compiler/operator_plan.rs:439` | compile-time | UNREACHABLE: outer `match (kind, operand)` arm restricts `kind` to the five string operators | y |
| `seclang/compiler/operator_plan.rs:457` | compile-time | UNREACHABLE: outer arm restricts `kind` to Eq/Ge/Gt/Lt | y |
| `seclang/compiler/generation.rs:276` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `seclang/compiler/generation.rs:278` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `seclang/compiler/generation.rs:384` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `seclang/tag_selection.rs:490` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `seclang/tag_selection.rs:497` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `seclang/directive.rs:296` | compile-time | UNREACHABLE: selector count was produced by checked parser arithmetic | y |
| `seclang/directive.rs:300` | compile-time | UNREACHABLE: selector range indexes the immutable directive document the parser wrote | y |
| `seclang/directive.rs:309` | compile-time | UNREACHABLE: selector count was produced by checked parser arithmetic | y |
| `seclang/directive.rs:313` | compile-time | UNREACHABLE: selector range indexes the immutable directive document the parser wrote | y |
| `seclang/target.rs:267` | compile-time | UNREACHABLE: `checked_slice` is called only with (start, count) records the parser wrote after `checked_add` succeeded; callers pass the invariant as `reason` | y |
| `seclang/target.rs:268` | compile-time | UNREACHABLE: `checked_slice` is called only with (start, count) records the parser wrote after `checked_add` succeeded; callers pass the invariant as `reason` | y |
| `seclang/action.rs:163` | compile-time | UNREACHABLE: retained charge was checked-added during parse | y |
| `seclang/action.rs:182` | compile-time | UNREACHABLE: list index written by the same parser into an immutable arena | y |
| `seclang/action.rs:202` | compile-time | UNREACHABLE: `checked_slice` is called only with (start, count) records the parser wrote after `checked_add` succeeded; callers pass the invariant as `reason` | y |
| `seclang/action.rs:203` | compile-time | UNREACHABLE: `checked_slice` is called only with (start, count) records the parser wrote after `checked_add` succeeded; callers pass the invariant as `reason` | y |
| `seclang/operator.rs:125` | compile-time | UNREACHABLE: private operator mapping index from the same parse | y |
| `seclang/operator.rs:277` | compile-time | UNREACHABLE: escape count checked at parse | y |
| `seclang/operator.rs:280` | compile-time | UNREACHABLE: escape range into the immutable document | y |
| `seclang/operator.rs:367` | compile-time | UNREACHABLE: subrange boundaries come from the char-aware tokenizer | y |
| `seclang/semantic/model.rs:561` | compile-time | UNREACHABLE: decoded arena is built only from `&str` pieces | y |
| `seclang/semantic/model.rs:596` | compile-time | UNREACHABLE: decoded arena is built only from `&str` pieces | y |
| `seclang/semantic/validate.rs:152` | compile-time | UNREACHABLE: `MissingId` is raised before finalization for id-less roots | y |
| `seclang/semantic/validate.rs:207` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors the documents 1:1 | y |
| `seclang/semantic/validate.rs:241` | compile-time | UNREACHABLE: `affected`/`index` contain only roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:247` | compile-time | UNREACHABLE: `affected`/`index` contain only roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:272` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:276` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:280` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:284` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:288` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:292` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:307` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:311` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:316` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:320` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:324` | compile-time | UNREACHABLE: two-pass count/validate-then-commit: the same sum was checked against its limit in the allocation-free pass | y |
| `seclang/semantic/validate.rs:326` | compile-time | UNREACHABLE: `affected`/`index` contain only roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:443` | compile-time | UNREACHABLE: `affected`/`index` contain only roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:446` | compile-time | UNREACHABLE: `MissingId` is raised before finalization for id-less roots | y |
| `seclang/semantic/validate.rs:464` | compile-time | UNREACHABLE: `affected`/`index` contain only roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:638` | compile-time | UNREACHABLE: the enclosing `end - cursor > 1` guard admits only groups of two or more entries | y |
| `seclang/semantic/validate.rs:639` | compile-time | UNREACHABLE: the enclosing `end - cursor > 1` guard admits only groups of two or more entries | y |
| `seclang/semantic/validate.rs:663` | compile-time | UNREACHABLE: a child is always preceded by its root (`ChainWithoutRoot` rejected earlier) | y |
| `seclang/semantic/validate.rs:679` | compile-time | UNREACHABLE: every root fold is assigned a disruptive behavior during folding | y |
| `seclang/semantic/validate.rs:732` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors the documents 1:1 | y |
| `seclang/semantic/validate.rs:892` | compile-time | UNREACHABLE: `affected`/`index` contain only roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:1103` | compile-time | UNREACHABLE: selector kind Regex implies a regex body; kinds were compared equal above | y |
| `seclang/semantic/validate.rs:1107` | compile-time | UNREACHABLE: selector kind Regex implies a regex body; kinds were compared equal above | y |
| `seclang/semantic/validate.rs:1112` | compile-time | UNREACHABLE: selector kind Literal/Path implies selector text; kinds were compared equal above | y |
| `seclang/semantic/validate.rs:1116` | compile-time | UNREACHABLE: selector kind Literal/Path implies selector text; kinds were compared equal above | y |
| `seclang/semantic/validate.rs:1119` | compile-time | UNREACHABLE: `SelectorKind::None` returned `Ok(true)` before this match | y |
| `seclang/semantic/validate.rs:1161` | compile-time | UNREACHABLE: `affected`/`index` contain only roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:1274` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors the documents 1:1 | y |
| `seclang/semantic/validate.rs:1302` | compile-time | UNREACHABLE: replayed roots come from the same index of roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:1307` | compile-time | UNREACHABLE: replayed roots come from the same index of roots with `unit: Some` | y |
| `seclang/semantic/validate.rs:1312` | compile-time | UNREACHABLE: replayed roots come from the same index of roots with `unit: Some` | y |
| `seclang/transaction/predicate.rs:18` | request-path | UNREACHABLE: `tag_rule_refs[*].tag` ranges were validated and sorted with the same accessor at compile time; `generation.bytes` is immutable | y |
| `seclang/transaction/predicate.rs:22` | request-path | UNREACHABLE: `tag_rule_refs[*].tag` ranges were validated and sorted with the same accessor at compile time; `generation.bytes` is immutable | y |
| `seclang/semantic/template.rs:469` | compile-time | UNREACHABLE: two-pass discipline: the allocation-free count pass already returned `Err` for invalid templates | y |
| `seclang/semantic/template.rs:520` | compile-time | UNREACHABLE: two-pass discipline: the allocation-free count pass already returned `Err` for invalid templates | y |
| `seclang/semantic/template.rs:585` | compile-time | UNREACHABLE: two-pass discipline: the allocation-free count pass already returned `Err` for invalid templates | y |
| `seclang/semantic/scan.rs:45` | compile-time | UNREACHABLE: scan index is bounded by the immutable document length | y |
| `seclang/semantic/scan.rs:91` | compile-time | UNREACHABLE: an explicit operator name always has a span and range | y |
| `seclang/semantic/scan.rs:93` | compile-time | UNREACHABLE: an explicit operator name always has a span and range | y |
| `seclang/semantic/scan.rs:100` | compile-time | UNREACHABLE: an explicit operator name always has a span and range | y |
| `seclang/semantic/scan.rs:238` | compile-time | UNREACHABLE: the registry is a static table containing canonical spellings | y |
| `seclang/semantic/fold.rs:62` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors `documents[*].len()` 1:1 | y |
| `seclang/semantic/fold.rs:165` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors `documents[*].len()` 1:1 | y |
| `seclang/semantic/fold.rs:224` | compile-time | UNREACHABLE: `classify_transformation` keeps the operand for every classified action | y |
| `seclang/semantic/fold.rs:409` | compile-time | UNREACHABLE: the enclosing `is_disruptive(kind)` arm admits only Block/Deny/Pass | y |
| `seclang/semantic/fold.rs:632` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors `documents[*].len()` 1:1 | y |
| `seclang/semantic/fold.rs:732` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors `documents[*].len()` 1:1 | y |
| `seclang/semantic/fold.rs:913` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors `documents[*].len()` 1:1 | y |
| `seclang/semantic/fold.rs:1014` | compile-time | UNREACHABLE: fold roles are assigned for every rule in the first pass of `commit_final_graph` | y |
| `seclang/semantic/fold.rs:1038` | compile-time | UNREACHABLE: a child is always preceded by its root (`ChainWithoutRoot` rejected earlier) | y |
| `seclang/semantic/fold.rs:1044` | compile-time | UNREACHABLE: a root phase state is Explicit or RequiresDefault; only children inherit | y |
| `seclang/transaction/operators.rs:238` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `matcher.rs:2495` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `matcher.rs:2569` | n/a | NOT A PANIC SITE: `#[expect(...)]` lint attribute matched by the pattern | n/a |
| `seclang/semantic/draft.rs:78` | compile-time | UNREACHABLE: iterator index < len | y |
| `seclang/semantic/draft.rs:98` | compile-time | UNREACHABLE: retained sum was `checked_add`-validated against `max_retained_bytes` in the constructor | y |
| `seclang/semantic/graph.rs:690` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors the documents 1:1 | y |
| `seclang/semantic/graph.rs:694` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors the documents 1:1 | y |
| `seclang/semantic/graph.rs:832` | compile-time | UNREACHABLE: directive indices come from `draft.directives`, which mirrors the documents 1:1 | y |
| `seclang/semantic/operand.rs:662` | compile-time | UNREACHABLE: two-pass discipline: the preceding allocation-free pass validated the grammar, ranges and shape | y |
| `seclang/semantic/operand.rs:694` | compile-time | UNREACHABLE: two-pass discipline: the preceding allocation-free pass validated the grammar, ranges and shape | y |
| `seclang/semantic/operand.rs:721` | compile-time | UNREACHABLE: two-pass discipline: the preceding allocation-free pass validated the grammar, ranges and shape | y |
| `seclang/semantic/operand.rs:765` | compile-time | UNREACHABLE: two-pass discipline: the preceding allocation-free pass validated the grammar, ranges and shape | y |
| `seclang/semantic/operand.rs:770` | compile-time | UNREACHABLE: two-pass discipline: the preceding allocation-free pass validated the grammar, ranges and shape | y |
| `seclang/semantic/operand.rs:774` | compile-time | UNREACHABLE: two-pass discipline: the preceding allocation-free pass validated the grammar, ranges and shape | y |
| `seclang/semantic/operand.rs:791` | compile-time | UNREACHABLE: two-pass discipline: the preceding allocation-free pass validated the grammar, ranges and shape | y |
| `seclang/semantic/operand.rs:828` | compile-time | UNREACHABLE: template-capable operators returned before static classification | y |
| `seclang/semantic/actions.rs:88` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:182` | compile-time | UNREACHABLE: `SetvarOperation` arithmetic variants always carry a value range | y |
| `seclang/semantic/actions.rs:418` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:422` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:424` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:542` | compile-time | UNREACHABLE: the ctl envelope was validated to contain `=` | y |
| `seclang/semantic/actions.rs:740` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:749` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:759` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:762` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:763` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:779` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:780` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:796` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:798` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:805` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |
| `seclang/semantic/actions.rs:808` | compile-time | UNREACHABLE: two-pass discipline: the preceding validation pass required the operand/template/mapping | y |

Direct indexing on request-derived indices (not matched by the pattern) was checked by S5: the only
reachable case was F-S5-1; the others (`body.rs` KMP prefix table and delimiter cursor, `transform.rs`
`index + n < len` guards, `xml_stream.rs` scan guards, `json_inspection.rs` profile leases, Aho-Corasick
state tables, `xss/blacklists.rs` 256-entry tables, `sqli/mod.rs` equal-length compares) are guarded.

## Alternatives and limits

- Reviewed and not filed: F-S1-4 (tag index sort comparisons uncharged; bounded by source size x log n),
  F-S1-5 (two-pass selection budget verified correct), F-S3-5 (urlencoded splits only on `&`; equals
  ModSecurity, differs only from PHP-family backends, payload stays visible in one value), S4 operator notes
  (`@contains` with an empty expanded operand follows Coraza, `@validateUtf8Encoding` is stricter than
  ModSecurity, TX captures beyond `TX.9` are a superset), F-S5-2 (part of waf3-01), F-S5-3 (part of waf-02).
- Verification limits: source-derived for S1-S3 and S5 except the F-S5-1 PoC; S2 and S3 reference
  behavior is partly from memory of ModSecurity/Coraza sources (per-row confidence in the finding files);
  the S4 oracle is the pinned Go port, not C libinjection; no go-ftw differential against ModSecurity or
  Coraza was run; no gateway-level load or panic-containment test; x86-64 hardware not covered.
- Line numbers in table (c) are valid at the commit that adds this page only; re-run the command.

## Verification and re-evaluation

- Fresh commands at the commit adding this page: `cargo test -p edgion-waf` (917 lib tests, 0 failed), the
  `rg` command above (126 hits), `cargo clippy -p edgion-waf --all-targets` clean.
- Re-evaluate the affected rows when: a transformation is added or changed; a body processor is added or a
  parser's strictness changes; an operator is added; the regex engine or Aho-Corasick backend changes; a
  new `expect` / `unreachable!` lands on the request path; the gateway changes body-processor selection or
  adds a panic boundary; libinjection-go is re-pinned.

## Resolution references

- 2026-11-07 second-pass lenses S1-S5. Fixes: F-S5-1 `085fab687`; waf4-01 / F-S4-1 `5dacb60b7`; waf-03 /
  F-S2-1 `2ac1fc9c8`; F-S1-1/2/3 `53229a191`. Open findings: waf3-01, waf3-02, waf3-03, waf4-02, waf5-01..07.
