---
id: ARCH-037
status: candidate
kind: behavior-defect
priority: P2
partition: PART-332
analyst: /root
independent_verification: VERIFICATION-ARCH-037.json
---

# Substitute ProxyRewrite path parameters at their matched token boundaries

## Concrete trigger and ownership

`ProxyRewrite::resolve_variables` collects exact regex matches for path parameter
names, then applies `result.replace(full_match, replacement)` across the evolving
entire string for each collected match. The replacement operation ignores the
lexical boundary established by `PATH_PARAM_PATTERN`.

For template `$id/$id2` with routed parameters id=first and id2=second, the first
replacement changes both `$id` and the prefix of `$id2`, producing `first/first2`.
The later exact `$id2` replacement finds no remaining token. The same issue affects
URI and add/set header values; it can forward the wrong configured routing or
identity field. Longer-token-first order can hide the defect. No production
incident or security bypass is asserted.

Production `pg_request_filter` freezes named parameters from the matched route
and normalized match-time path through `ctx.capture_matched_path_params` before
plugins run. `get_path_param` reads that init-once cache; adapter lazy extraction
is a fallback for synthetic/test contexts that bypass the production hook.
Distinct names such as id and id2 are retained independently. The bug is the
downstream textual replacement, not radix matching or parameter lookup. Existing
single-name tests such as `test_path_param_substitution_url_encodes_in_header_context`
do not distinguish correct token replacement from global substring replacement.

## Bounded improvement and alternative

Replace each original matched path-parameter span in the input to the final
path-parameter step exactly once in one local pass, preserving unchanged spans
and the existing URL encoding of each resolved value. Do not perform a global
string replacement for a token discovered by the lexer. A regex replacement
callback or explicit span builder can own this narrow step without the
intermediate Vec of owned token/name Strings and repeated full rescans. It must
also respect escaped-dollar positions; an ordinary callback does not establish
that condition automatically. Sorting by name length is an incomplete workaround
and keeps unnecessary ordering policy instead of respecting match boundaries.

Retain the separate ProxyRewrite `$uri`/`$arg_`/capture language. The canonical
`${type:name}` template contract explicitly excludes this dialect; migrating its
public syntax or creating a second general template engine is not required.

## Required invariants and future verification

Preserve `$uri`, query argument and capture substitution order; reserved uri and
arg_ name handling; unresolved matches remaining literal in the final
path-parameter step; repeated exact names; empty values; and ASCII-safe percent
encoding. Preserve the production match-time cache rather than extracting again
after an earlier rewrite or adding another request-path cache.

`escape_dollar` documents a nonrecursive injection intention, but the current
PATH_PARAM_PATTERN still matches dollar text preceded by its NUL sentinel. A
plain span callback fixes prefix corruption while still interpreting those
escaped positions. The final path-parameter pass must leave them literal until
the existing restoration step. This does not establish that earlier `$uri`,
argument or capture stages handle every boundary or escape case correctly, and
does not authorize redesigning those stages. Future tests should distinguish the
prefix correction from any independently observed earlier-stage escape defect.

Keep original-query preservation, regex URI versus ordinary template encoding,
URI priority, normalized path/session finalization, explicit Host/method changes,
ordered add/set/remove headers, bounded JSON patch work/output and atomic canonical
request-body publication. No response aggregation, session escape hatch or new
request-path cache is introduced.

Future network-free tests should cover id/id2 in both orders, repeated occurrences,
unknown/empty names, reserved names, encoding and injected dollar values. Cover
both URI and header consumers and assert values rather than helper call counts.
Source inspection establishes the deterministic substring substitution; no tests,
builds, patch prototypes, network or performance measurements were run.

Original normalization/radix and request-template records, the production
match-time capture hook and adapter fallback, resource precompile and actual
ProxyRewrite patch/variable tests were inspected.
REPORT-PART-332.json records exact primary hashes and selected support scope.
Independent verification is pending.
