---
name: oidc-authentication-response-byte-caps
description: Review token exchange, refresh, and introspection response byte limits and bounded failure classification.
knowledge_type: regression-guards
---

# OIDC authentication response byte caps

## Conclusion and scope

The Gateway OIDC plugin bounds token exchange, refresh, and introspection bodies
to 1 MiB, including JSON whitespace. `read_oidc_json` checks the known body
length before reading, delegates accumulation to the existing
`common::outbound::call::stream_body_capped`, rejects its truncation flag, and
only then deserializes. Exactly 1 MiB is accepted. There is no plugin-local
buffering loop or new operator field.

`TokenEndpointResponse` retains the access/ID/refresh token fields and expiry;
introspection still validates `active`, issuer, audience, expiry, and scopes
before caching claims. `OidcDependencyFailure::{ResponseTooLarge, ResponseRead,
ResponseJson}` carries no provider data. `OidcVerifyError::Dependency` maps to static PluginLog tags and
502 / `Bad Gateway` when the caller emits an upstream failure. Existing session
refresh fallbacks and `unauthAction` remain unchanged; a failed preemptive refresh
does not invalidate an otherwise valid existing token.
In Auto mode, failed introspection following an invalid JWT keeps the original
401 / `Invalid token format` result. `InvalidFormatAfterIntrospection` retains
only the bounded dependency classification for the caller's PluginLog.

## Rationale and evidence

Canonical authority: repository `AGENTS.md`, HTTP body handling invariant 3,
requires an explicit streaming cap on plugin-initiated exchanges. This is Edgion's
reqwest path, not the main Pingora upstream response path.

Source D0036 / `oidc-auth-response-byte-caps.md`: code exchange and refresh used
uncapped `Response::json`; the consumer audit also found uncapped introspection.
Timeouts, cache capacity, and refresh singleflight do not bound one response.
The fixed 1 MiB limit follows the project's usual independent-exchange limit,
without introducing a resource schema knob.

| Consumer | Body handling |
|---|---|
| Authorization-code exchange, refresh | `token_exchange.rs`: `read_auth_response` delegates to `read_oidc_json`, fixed 1 MiB |
| Introspection | `token_verify.rs`: same reader, fixed 1 MiB, before claims checks/cache |
| Discovery | `jwks_provider.rs`: `read_oidc_json`, `discovery_max_response_bytes` |
| JWKS | `jwks_provider.rs`: `read_oidc_json`, `jwks_max_response_bytes` |
| Userinfo | `userinfo.rs`: `read_oidc_json`, `userinfo_max_response_bytes` |
| Revocation | `token_exchange.rs`: status only, response body dropped |
| Authorization and end-session | Browser redirect construction; no Gateway HTTP response body read |
| Non-success HTTP responses | Rejected by status before aggregation |

## Alternatives and limits

ARCH-031 retired the orphan `common::http_util::fetch_json_limited` helper,
its public module export, and four socket-based unit tests. A fresh workspace
Rust caller audit found only those four test calls and no production consumers.
The serving OIDC reader already composed the canonical capped body helper with
typed error classification. Keeping the orphan under `cfg(test)` would retain
a disconnected algorithm; delegating it would retain an unused String-error API.
Its formatted transport/parser messages retire with that API. The active typed
failure and static public/log contracts remain covered by in-memory unit tests.
No endpoint admission, TLS, redirects, timeout, client authentication, SAN metric,
cache, singleflight, or revocation policy changes are involved.

The cap bounds retained body bytes, not all process memory: the incoming chunk,
allocator capacity, and parsed JSON representation also consume memory.
The cap correction alone did not establish full OIDC public-error redaction.
The later [dependency projection correction](dependency-error-projection.md)
covers other dependency and status paths and generalizes the same capped reader.

## Verification and re-evaluation

Before the fix on 2026-09-09, six explicit network integration tests accepted
exact-limit valid JSON, then failed because 1 MiB + 1 byte was also accepted.
They call all three real endpoint methods, each with Content-Length and chunked
framing. Their fixture is explicitly ignored in ordinary unit runs and does not
modify the existing integration harness or tests.

Historical verification on 2026-09-09 (not a fresh check of the later error-model change):

- `cargo test -p edgion-gateway --lib auth_response_caps_integration -- --ignored`:
  8 network integration tests, including header-only early rejection and the
  real Auto JWT-to-introspection fallback preserving 401 and its log reason.
- The freshly built Gateway test binary filtered by
  `plugins::http::openid_connect`: 133 passed, 8 explicit network tests ignored
  (executed separately above). Earlier module runs also passed via Cargo.
- Independent read-only review: LGTM after retaining Auto diagnostics and removing
  the redundant plugin name from new log tags. The Auto fixture was corrected
  to use valid HS256 signing with the wrong JSON claims shape, rather than a
  token that bypassed JWT verification or failed with InvalidSignature.

Final quick checks passed: `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
agent-doc consistency, SSA-force, metrics inventory, Pingora source policy,
Gateway no-tracing, and `git diff --check`. Clippy retains existing workspace
warnings; the added test-module placement warning was corrected before the
final successful run. No full workspace test suite was run.

Focused in-memory reader tests cover exact
limit, oversize rejection without polling the remaining stream, read failure
after a valid JSON prefix, malformed JSON and wrong field shape, and static
public/log error mappings. Re-evaluate when an OIDC HTTP consumer is added,
the shared reader changes, or a provider requires a different documented limit.

ARCH-031 retains all six existing tests in
`token_exchange_test/response_tests.rs` and adds an 8-byte caller-selected cap
case (exact-limit success, known-size overflow, and unknown-size overflow) plus
a pending-reader cancellation case. The latter polls through a valid JSON prefix,
verifies the reader remains pending and its stream remains live, then verifies
dropping the reader destroys the response stream. Both use synthetic reqwest
responses with existing dependencies, private test access, and no network or
timer. These cases exercise the OIDC composition; they do not establish real
transport behavior or a process-wide heap bound. The historical check results
above are not verification of this retirement.

| Retired legacy case | Current in-memory replacement |
|---|---|
| `ok_within_limit` | `exact_limit_stream_preserves_token_fields`; `caller_selected_small_cap_is_enforced` |
| `content_length_rejects_early` | `known_oversize_body_is_rejected`; `caller_selected_small_cap_is_enforced` |
| `stream_rejects_dishonest_chunked` | `stream_stops_at_first_oversize_chunk_without_parsing_prefix`; `caller_selected_small_cap_is_enforced` |
| `parse_error_surfaces_with_ctx` | `malformed_or_wrong_shape_json_has_no_provider_detail`; `response_errors_keep_wire_and_contextual_log_contracts` |

The known-size replacement verifies oversize classification before parsing; it
does not instrument zero body polling. The retained stream-overflow test verifies
that the remaining stream is not polled after overflow. The legacy parse/context
String assertion is replaced by the serving API's typed failure and static
operation-specific log contract. `body_read_failure_is_static_and_rejects_valid_prefix`
also remains unchanged, and existing explicitly ignored transport fixtures remain
available separately.

## Resolution references

- D0036 credential ownership audit; source task `oidc-auth-response-byte-caps.md`,
  fixed on 2026-09-09.
- ARCH-031: orphan JSON reader retirement and network-free replacement coverage.
- Production: `edgion-gateway/src/plugins/http/openid_connect/{token_exchange,token_verify,error,callback,request_filter_impl}.rs`.
- Reproduction: `edgion-gateway/tests/unit/plugins/http/openid_connect/plugin_test/auth_response_caps_integration.rs`.
- In-memory coverage: `edgion-gateway/tests/unit/plugins/http/openid_connect/token_exchange_test/response_tests.rs`.
