# ProxyRewrite staged template compilation

Status: deferred for user decision. No runtime change.
Source baseline: 325277e61.

## Evidence and scope

`ProxyRewrite::resolve_variables` uses shared LazyLock regexes, not per-request
regex compilation. It still scans and materializes the evolving result through
URI, argument, numeric-capture and named-path stages. Recent ownership changes
remove intermediate copies but do not eliminate these scans. No request-level
bottleneck or throughput benefit has been measured.

A plan compiled only from configured token spans is not automatically equivalent
to the current implementation:

- The literal `$uri` replacement also consumes the prefix of `$uri2`.
- Inserted raw URI text is dollar-escaped with NUL, but the following argument
  and numeric regex passes do not inspect that sentinel. Only the final named
  path pass skips sentinel-prefixed dollars. Replacing an introduced token may
  leave a NUL behind, so compare actual sink acceptance/errors as well as text.
- The raw regex-URI branch replaces numbered captures sequentially in ascending
  order. A `$1` replacement can consume the prefix of `$10` before capture ten
  is considered. The resulting text then enters the ordinary staged resolver.

These are source observations, not accepted requirements. Comments describing
general prevention of re-interpretation overstate the implementation. The owning
knowledge page already limits its guard to the final named-path stage; its
historical acceptance cannot authorize preserving or changing the earlier
behavior without resolving the intended contract.

The canonical `${...}` engine compiles configuration segments and treats inserted
values as literal data. Its architecture explicitly excludes this `$uri` dialect.
Do not migrate blindly or introduce a second general template engine as a
performance-only change. JSON `valueTemplate` already uses the canonical engine
and is outside this proposal.

## Decision requested

Choose a bounded, measured optimization that preserves current staged behavior,
or authorize a separate semantic design for non-recursive inserted values and
unambiguous token boundaries before compiling this dialect. Retaining current
behavior is also an option if measured savings do not justify the complexity.

The preserving option may precompute immutable configuration facts, but must
still account for runtime-introduced tokens; it cannot assume a configured token
inventory describes all later stages. The semantic option must state exact
token grammar, unknown/missing/empty behavior, sentinel policy and numeric-capture
boundaries. Edgion is pre-release: do not add a compatibility mode or legacy shim
without an explicit requirement.

## Acceptance before implementation

Use IMPL with an exact source allow-list and reviewed ownership design. Measure
scanning, allocations/copied bytes and retained compiled metadata for literal,
variable-heavy and long templates before claiming value. Preserve immutable
configuration generations and old in-flight plugin instances.

Tests must cover URI and add/set header sinks, frozen query/path inputs,
absent/empty values, percent encoding, repeated/unknown/prefix-related names,
injected `$uri`, `$arg_`, numeric and named tokens, existing NUL, capture ten,
raw regex-capture slashes, query preservation and failure/log behavior. A parser
output fixture alone does not prove what the URI/header setter accepts. Keep
URI priority and header add/set/remove ordering explicit. Do not touch body
mutation, logging ownership or schema in an incidental optimization.

## Sources and verification limits

- `edgion-gateway/src/plugins/http/proxy_rewrite/plugin.rs`: patterns,
  `resolve_variables`, `rewrite_uri`, `modify_headers`.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs`:
  `RegexUri` and configuration preparation.
- `skills/01-architecture/02-gateway/request-template-runtime.md`.
- `skills/04-review/plugin-runtime/regression-guards/proxy-rewrite-path-token-boundaries.md`.

This record is a source audit and design decision request, not a runtime
reproduction, benchmark or endorsement of the observed substitution quirks.
