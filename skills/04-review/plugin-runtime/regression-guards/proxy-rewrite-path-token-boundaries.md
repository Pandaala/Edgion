---
name: proxy-rewrite-path-token-boundaries
description: Use when reviewing ProxyRewrite named path substitutions, prefix-related token names, and escaped injected dollars.
knowledge_type: regression-guards
---

# ProxyRewrite named path tokens preserve their spans

## Conclusion and scope

`ProxyRewrite::resolve_variables` owns the separate `$uri`, `$arg_`, capture and
named-path dialect used by URI and add/set request-header rewriting. Its final
named-path pass replaces each original matched span once, skipping dollars
preceded by the existing NUL escape sentinel. Complete names are independent:
`$id/$id2` becomes `first/second`, regardless of occurrence order. Repeated tokens
all resolve; unknown names remain literal; present empty values become empty.
Resolved values retain the existing ASCII-safe percent encoding in both contexts.

This dialect remains outside the [canonical `${...}` request-template runtime](../../../01-architecture/02-gateway/request-template-runtime.md).
No parser, session capability, schema, request cache or new template engine is needed.

## Rationale and evidence

ARCH-037 identified the former collected owned token/name vector followed by
String::replace on the evolving full result. Replacing `$id` also changed `$id2`
and unknown `$id3`, making later names and occurrence order affect output.
An exact-span replacement callback borrows unchanged text from the immutable
stage input; encoded replacement values are owned. It checks the byte before the
matched dollar so named tokens injected through `$uri` and raw regex URI captures
survive restoration as literal data beside original resolvable tokens.

The production `pg_request_filter` hook freezes parameters via
`EdgionHttpContext::capture_matched_path_params` before plugins; the adapter reads
that init-once cache. Its lazy extraction is a fallback for synthetic contexts
that bypass the hook. Capture ownership and rewrite-time normalized paths are
unchanged. All URI changes still go through `PluginSession::set_upstream_uri`.

## Alternatives and limits

Sorting names by length does not protect unknown longer names and retains an
ordering policy. A callback without the sentinel check repairs prefix corruption
but still interprets escaped injected text. The correction remains local to the
final named-path pass; earlier `$uri`, argument and numeric-capture stages and
regex URI pre-substitution keep their separate limitations. In particular,
`$uri2` is already changed by the earlier `$uri` stage, and injected `$arg_` or
numeric references may be processed before this guard applies. This record does
not certify those stages or permit broader recursive substitution changes.

Original query preservation, URI priority, regex URI raw-capture slash behavior,
header order, Host/method handling and bounded atomic JSON mutation are unchanged.
Removing the intermediate token vector and repeated global replacement is a
structural fact; no throughput, allocation-count or memory improvement is measured.

## Verification and re-evaluation

Regressions live in `edgion-gateway/tests/unit/plugins/http/proxy_rewrite/plugin_test.rs`:
`path_tokens_use_exact_original_spans_in_both_contexts`,
`path_tokens_preserve_injected_dollars_beside_original_tokens`,
`path_tokens_reach_ordinary_uri_and_add_set_header_sinks`, and
`regex_uri_raw_captures_keep_named_dollars_literal`. They assert URI and header
values, prefix-related names in both orders, repeats, unknown/empty/nonmatching
text, encoding and escaped dollars next to original tokens. Sink tests preserve
original queries and validate encoded header bytes. Existing tests retain identities.
The executable original implementation passed all 13 existing tests and failed all
four new regressions. After correction, `cargo test -p edgion-gateway --lib
plugins::http::proxy_rewrite` passed all 17 tests. Project-gate results are recorded
in the implementation workflow evidence; historical checks elsewhere are not
fresh verification of this correction.
No network, live hot reload, full workspace test suite or benchmark is implied.
Re-evaluate when token grammar, stage order, escaping, encoding, session missing-value
semantics, or match-time capture ownership changes.

The argument, numeric-capture and named-path stages accept owned regex replacement
results while retaining their input String when the regex returns an unchanged
borrowed result. Final dollar restoration consumes its String and
returns that allocation unchanged when no NUL sentinel is present. Sentinel
replacement still removes only NUL immediately followed by `$`; stray NUL bytes
retain their existing behavior. The direct restoration test checks output and
allocation identity, while the existing both-context and sink tests guard stage
semantics. This does not remove all intermediate substitution allocations.

Final URI query assembly also consumes the resolved path String. Absent or empty
original queries return it unchanged; nonempty queries append the existing `?`
or `&` separator and literal query bytes. Spare capacity is reused, otherwise
the buffer may grow. Direct tests cover duplicate/encoded query fields, an
existing query, a trailing question mark and empty paths without normalizing
these inputs; both template and regex URI sinks retain their existing tests.

Argument and path substitutions borrow the canonical session getters until
percent encoding produces owned output. These read the same frozen query and
init-once path-capture caches as the owned getters; missing arguments still
become empty, while missing path names stay literal and present empty captures
become empty. The combined both-context regression forbids owned getter calls.
Initial cache population and encoded output can still allocate.

Dollar escaping preserves its input ownership with `Cow<str>` when no dollar
occurs: raw path/capture inputs stay borrowed, and encoded Strings transfer
without another copy. Inputs containing dollars retain the same NUL insertion.
Direct tests assert borrowed/owned identity and exact escaped/restored bytes,
including existing NUL and repeated dollars. This does not alter stage ordering
or the earlier-stage limitations described above.

## Resolution references

- `ARCH-037` — final-stage original-span replacement and escaped-dollar boundary.

## Combined integration registration

`ProxyRewriteTestSuite::test_cases` includes the existing `combined` case exactly
once alongside the nine URI/template cases (ARCH-386). Both fixture trees route
`/combined` to `proxy-rewrite-combined`; its assertions observe Host replacement,
X-Gateway addition, a URI-derived X-Original-Path value, and removal of the two
sent header markers. A declared fixture and an unregistered case do not establish
execution coverage. This registration correction leaves the assertion body and
fixture topology unchanged; source/fixture checks are not live integration or
mutation-test results. Multi-value append remains covered by its separate
canonical regression, and this case does not assert X-Request-Id.
