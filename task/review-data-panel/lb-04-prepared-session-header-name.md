# Evaluate prepared configured session-persistence header names

Status: deferred optimization for user decision. No runtime change.
Inspected baseline: `7bf5a0100`.

## Evidence and decision

The Header issuance branch in
`edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_filter.rs`
copies `get_session_name(sp)` into a String before `append_header`.
The policy and compiled lifetime come from the same request-pinned HTTP or
gRPC rule generation through `effective_session_persistence_with_lifetime`.
Configured names remain resource-owned Strings.

The pinned Edgion Pingora source at `1aaa45cc70430bc782990bc20ddee3fef36d6b99`
implements IntoCaseHeaderName for String, Bytes, CaseHeaderName and static
strings, but not arbitrary borrowed strings. The String becomes original-case
Bytes. append_header_value separately parses a normalized HeaderName and, when
enabled, retains the original spelling in the case map. Replacing the String
with a borrowed HeaderName is not an equivalent zero-copy fix: Pingora applies
its title mapping for certain standard names and copies other normalized names.

Preparing original-case Bytes or CaseHeaderName at rule construction could
avoid repeating the configured-name payload copy during issuance. It retains
another owner beside the resource String and introduces shared-buffer ownership
across rule generations and responses. First-clone promotion and steady-state
clone costs need separate measurement. Pingora's normalized-name parsing and
header-map work would still remain. No candidate implementation, net memory
benefit or throughput improvement has been established.

Choose whether issuance frequency justifies that persistent storage and
ownership, or retain the current representation. The absent-name static default
is a separate, potentially mechanical optimization; this task does not defer
that branch. Explicitly configured empty names must not become defaults.
This is distinct from [route predicate header-name preparation](routing-04-header-name-preparation.md),
whose current HeaderMap lookup does not allocate a name buffer.

## Acceptance for a later implementation

- Publish prepared data with the same immutable effective HTTP/gRPC policy
  generation, including inherited/resolved rules and old requests during reload.
  Keep Pingora-specific types out of the resource schema and transport.
- Preserve original spelling for standard/custom/mixed-case names, explicit
  empty and invalid names, repeated-header append behavior and append failures.
  Do not introduce new validation or silently skip invalid names at construction.
  Current issuance ignores append errors and then consumes the injection state.
- Preserve post-retry issuance, interim and 101 exclusion, endpoint identity,
  token scope, expiry sampling, lifetime and Cookie behavior. Injected headers
  remain absent from a subsequently replaced local reply; see the original
  [local-reply decision](../../skills/04-review/routing/intentional-design/local-reply-drops-sticky-set-cookie.md)
  and [captured lifetime ownership](../../skills/04-review/routing/regression-guards/session-persistence-token-extraction.md).
- Compare current String transfer against prepared ownership with case maps both
  enabled and disabled. Measure first and repeated issuance, name lengths,
  allocation requests, retained bytes, construction/reload cost and live old
  generations. Include no-issuance routes; distinguish refcount work, parsing
  and HeaderMap allocation from the removed payload copy.
- Use existing canonical types, no global cache, request-time lazy preparation,
  new logging or metrics. Freeze an exact implementation allow-list and obtain
  independent IMPL plan/final review, targeted HTTP/gRPC issuance and publication
  tests, and repository checks.

Fresh baseline verification passed 26 session-persistence and 11 upstream
response-filter tests. These are in-process checks of existing behavior, not
comparative performance evidence or a live HTTP/2/conf_sync test.
