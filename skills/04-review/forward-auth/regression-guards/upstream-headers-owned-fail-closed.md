---
name: forward-auth-upstream-headers-owned-fail-closed
description: Use when reviewing ForwardAuth decision.upstreamHeaders copying, client-sent copies of those names, hideCredentials removal, or the response to a failed upstream-header mutation.
status: fixed
closed: 2026-11-09
knowledge_type: regression-guards
---

# ForwardAuth `upstreamHeaders` Are Plugin-Owned and Fail Closed

## Conclusion

Every name in `decision.upstreamHeaders` is a ForwardAuth-owned output (user
decision, auth identity-header failure design AC-9). On an authorizer success the
plugin first removes **every** configured name from the request, then copies the
authorizer response's value for each name. The authorizer request is already
built, so clearing does not change what the authorizer saw.

- A returned value is copied byte-for-byte as an `http::HeaderValue` (UTF-8 and
  other obs-text included; reqwest and the gateway share one `http` crate). A name
  the authorizer did not return stays absent. This includes a name the client sent itself, such as `Authorization`:
  listing it means the authorizer, not the client, supplies it upstream (matches
  Traefik `authResponseHeaders`). This is intended, not a regression.
- A remove or write failure, or a `hideCredentials` failure to remove
  `Authorization`, denies after `authFailureDelayMs` with a fixed plugin-owned
  `403` whose body is `Forbidden` (`text/plain`). The authorizer's success body
  and `decision.clientHeaders` are never relayed on this path. PluginLog records
  `identity header rejected` or `credential removal failed` (`push_err`), and the
  access-log diagnostic `AuthHeaderMutationRejected` is recorded. Forwarding with a
  stale client copy, or relaying the authorizer's 2xx body under a deny, is the
  bug this guard prevents.
- `hideCredentials` removes the client `Authorization` **before** the owned names
  are cleared and copied, so an `Authorization` listed in `upstreamHeaders` and
  returned by the authorizer is forwarded rather than deleted afterwards.
- Cleared names are absent for later plugins in the chain as well.
- `Host` and the framing/hop-by-hop names in
  `auth_realm::RESERVED_AUTH_OUTPUT_HEADER_NAMES` (`Content-Length`,
  `Transfer-Encoding`, `TE`, `Trailer`, `Expect`, `Connection`, `Keep-Alive`,
  `Proxy-Connection`, `Upgrade`) are rejected by the shared
  `validate_owned_header_names`: removing them on every pass would change request
  framing or connection handling. `Host` (ignoring ASCII case) is rejected at admission as an `upstreamHeaders`
  name, and every name must pass the canonical header-name check: header-map
  removal cannot remove HTTP/2 URI authority, and the Pingora adapter recreates
  `Host` for HTTP/1 upstreams, so `Host` cannot honor the absence contract.

`AuthHeaderMutationRejected` is a diagnostic, not a metric or a per-event alert
condition; operators alert on its rate through log-based alerting.

## Root cause and fix

Before 2026-11-09 the success path copied only the names the authorizer returned
and ignored write errors, so a client-sent copy of a configured name reached the
upstream whenever the authorizer omitted it, returned non-text bytes, or the write
failed. `ForwardAuth::apply_upstream_headers` now clears all names before
copying and returns the first mutation error; `deny_header_mutation` owns the
fixed 403.

## Evidence

- Code: `edgion-gateway/src/plugins/http/forward_auth/plugin.rs` —
  `apply_upstream_headers`, `deny_header_mutation`, the success branch of
  `handle_outbound_response`. Admission:
  `edgion-resources/src/resources/edgion_plugins/plugin_configs/forward_auth.rs`.
- Unit tests: `pass_clears_omitted_upstream_headers_and_copies_returned_bytes`,
  `hidden_client_credential_does_not_drop_authorizer_authorization`,
  `pass_header_mutation_failures_deny_with_fixed_forbidden` and
  `pass_header_mutation_deny_applies_failure_delay` in
  `edgion-gateway/tests/unit/plugins/http/forward_auth/plugin_test.rs`;
  `forward_auth_rejects_invalid_upstream_header_names` and
  `forward_auth_rejects_host_upstream_header_ignoring_case` in
  `edgion-resources/tests/unit/resources/edgion_plugins/plugin_configs/forward_auth_test.rs`.
- Limits: the mock tests do not exercise a gRPC-JSON-transcoded route end to end;
  the failure branches are covered by failure injection.

## Re-evaluation triggers

Re-evaluate if the decision schema gains explicit set/append/keep-client
operations for upstream headers, if ForwardAuth gains a multi-value upstream copy,
or if the deny path starts relaying authorizer content.

## Related

- [Deny responses preserve repeated headers](deny-response-repeated-headers-preserved.md) — the
  non-success `clientHeaders` path.
- [Default header forwarding](../intentional-design/default-header-forwarding.md) — what the
  authorizer receives.
- [Auth identity headers overwrite written names (D0037)](../../security/intentional-design/auth-identity-headers-overwrite-written-names.md)
  — configured `upstreamHeaders` names are owned outputs, so clearing them does not contradict it.
